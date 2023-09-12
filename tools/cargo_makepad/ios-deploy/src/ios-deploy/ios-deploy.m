//TODO: don't copy/mount DeveloperDiskImage.dmg if it's already done - Xcode checks this somehow

#import <CoreFoundation/CoreFoundation.h>
#import <Foundation/Foundation.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/sysctl.h>
#include <stdio.h>
#include <signal.h>
#include <getopt.h>
#include <pwd.h>
#include <dlfcn.h>
#include <time.h>

#include <netinet/in.h>
#include <netinet/tcp.h>

#include "MobileDevice.h"
#import "errors.h"
#import "device_db.h"

#define PREP_CMDS_PATH @"/tmp/%@/fruitstrap-lldb-prep-cmds-"
#define LLDB_SHELL @"PATH=/usr/bin /usr/bin/lldb -s %@"
/*
 * Startup script passed to lldb.
 * To see how xcode interacts with lldb, put this into .lldbinit:
 * log enable -v -f /Users/vargaz/lldb.log lldb all
 * log enable -v -f /Users/vargaz/gdb-remote.log gdb-remote all
 */
#define LLDB_PREP_CMDS CFSTR("\
    platform select remote-'{platform}' --sysroot '{symbols_path}'\n\
    target create \"{disk_app}\"\n\
    script fruitstrap_device_app=\"{device_app}\"\n\
    script fruitstrap_connect_url=\"connect://127.0.0.1:{device_port}\"\n\
    script fruitstrap_output_path=\"{output_path}\"\n\
    script fruitstrap_error_path=\"{error_path}\"\n\
    target modules search-paths add {modules_search_paths_pairs}\n\
    command script import \"{python_file_path}\"\n\
    command script add -f {python_command}.connect_command connect\n\
    command script add -s asynchronous -f {python_command}.run_command run\n\
    command script add -s asynchronous -f {python_command}.autoexit_command autoexit\n\
    command script add -s asynchronous -f {python_command}.safequit_command safequit\n\
    connect\n\
")

const char* lldb_prep_no_cmds = "";

const char* lldb_prep_interactive_cmds = "\
    run\n\
";

const char* lldb_prep_noninteractive_justlaunch_cmds = "\
    run\n\
    safequit\n\
";

const char* lldb_prep_noninteractive_cmds = "\
    run\n\
    autoexit\n\
";

NSMutableString * custom_commands = nil;

/*
 * Some things do not seem to work when using the normal commands like process connect/launch, so we invoke them
 * through the python interface. Also, Launch () doesn't seem to work when ran from init_module (), so we add
 * a command which can be used by the user to run it.
 */
NSString* LLDB_FRUITSTRAP_MODULE = @
    #include "lldb.py.h"
;

const char* output_path = NULL;
const char* error_path = NULL;

typedef struct am_device * AMDeviceRef;
mach_error_t AMDeviceSecureStartService(AMDeviceRef device, CFStringRef service_name, unsigned int *unknown, ServiceConnRef * handle);
mach_error_t AMDeviceCreateHouseArrestService(AMDeviceRef device, CFStringRef identifier, CFDictionaryRef options, AFCConnectionRef * handle);
CFSocketNativeHandle  AMDServiceConnectionGetSocket(ServiceConnRef con);
void AMDServiceConnectionInvalidate(ServiceConnRef con);

bool AMDeviceIsAtLeastVersionOnPlatform(AMDeviceRef device, CFDictionaryRef vers);
int AMDeviceSecureTransferPath(int zero, AMDeviceRef device, CFURLRef url, CFDictionaryRef options, void *callback, int cbarg);
int AMDeviceSecureInstallApplication(int zero, AMDeviceRef device, CFURLRef url, CFDictionaryRef options, void *callback, int cbarg);
int AMDeviceSecureInstallApplicationBundle(AMDeviceRef device, CFURLRef url, CFDictionaryRef options, void *callback, int cbarg);
int AMDeviceMountImage(AMDeviceRef device, CFStringRef image, CFDictionaryRef options, void *callback, int cbarg);
mach_error_t AMDeviceLookupApplications(AMDeviceRef device, CFDictionaryRef options, CFDictionaryRef *result);
int AMDeviceGetInterfaceType(AMDeviceRef device);
AMDeviceRef AMDeviceCopyPairedCompanion(AMDeviceRef device);
#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
unsigned int AMDeviceCopyDeveloperModeStatus(AMDeviceRef device, uint32_t *error_code);
#endif

int AMDServiceConnectionSend(ServiceConnRef con, const void * data, size_t size);
int AMDServiceConnectionReceive(ServiceConnRef con, void * data, size_t size);
uint64_t AMDServiceConnectionReceiveMessage(ServiceConnRef con, CFPropertyListRef message, CFPropertyListFormat *format);
uint64_t AMDServiceConnectionSendMessage(ServiceConnRef con, CFPropertyListRef message, CFPropertyListFormat format);
CFArrayRef AMDeviceCopyProvisioningProfiles(AMDeviceRef device);
int AMDeviceInstallProvisioningProfile(AMDeviceRef device, void *profile);
int AMDeviceRemoveProvisioningProfile(AMDeviceRef device, CFStringRef uuid);
CFStringRef MISProfileGetValue(void *profile, CFStringRef key);
CFDictionaryRef MISProfileCopyPayload(void *profile);
void *MISProfileCreateWithData(int zero, CFDataRef data);
int MISProfileWriteToFile(void *profile, CFStringRef dest_path);

bool found_device = false, debug = false, verbose = false, unbuffered = false, nostart = false, debugserver_only = false, detect_only = false, install = true, uninstall = false, no_wifi = false;
bool faster_path_search = false;
bool command_only = false;
char *command = NULL;
char const*target_filename = NULL;
char const*upload_pathname = NULL;
char *bundle_id = NULL;
NSMutableArray *keys = NULL;
bool interactive = true;
bool justlaunch = false;
bool file_system = false;
bool non_recursively = false;
char *app_path = NULL;
char *app_deltas = NULL;
char *device_id = NULL;
char *args = NULL;
char *envs = NULL;
char *list_root = NULL;
const char * custom_script_path = NULL;
char *symbols_download_directory = NULL;
char *profile_uuid = NULL;
char *profile_path = NULL;
int command_pid = -1;
int _timeout = 0;
int _detectDeadlockTimeout = 0;
bool _json_output = false;
NSMutableArray *_file_meta_info = nil;
int port = 0;    // 0 means "dynamically assigned"
pid_t parent = 0;
// PID of child process running lldb
pid_t child = 0;
// Signal sent from child to parent process when LLDB finishes.
const int SIGLLDB = SIGUSR1;
NSString* tmpUUID;
struct am_device_notification *notify;
CFRunLoopSourceRef fdvendor_runloop;

CFMutableDictionaryRef debugserver_active_connections;

uint32_t symbols_file_paths_command = 0x30303030;
uint32_t symbols_download_file_command = 0x01000000;
CFStringRef symbols_service_name = CFSTR("com.apple.dt.fetchsymbols");
const int symbols_logging_interval_ms = 250;

const size_t sizeof_uint32_t = sizeof(uint32_t);

// Error codes we report on different failures, so scripts can distinguish between user app exit
// codes and our exit codes. For non app errors we use codes in reserved 128-255 range.
const int exitcode_timeout = 252;
const int exitcode_error = 253;
const int exitcode_app_crash = 254;

// Checks for MobileDevice.framework errors, tries to print them and exits.
#define check_error(call)                                                       \
    do {                                                                        \
        unsigned int err = (unsigned int)call;                                  \
        if (err != 0)                                                           \
        {                                                                       \
            const char* msg = get_error_message(err);                           \
            NSString *description = msg ? [NSString stringWithUTF8String:msg] : @"unknown."; \
            NSLogJSON(@{@"Event": @"Error", @"Code": @(err), @"Status": description}); \
            on_error(@"Error 0x%x: %@ " #call, err, description);               \
        }                                                                       \
    } while (false);

// Checks for MobileDevice.framework errors and tries to print them.
#define log_error(call)                                                       \
    do {                                                                        \
        unsigned int err = (unsigned int)call;                                  \
        if (err != 0)                                                           \
        {                                                                       \
            const char* msg = get_error_message(err);                           \
            NSString *description = msg ? [NSString stringWithUTF8String:msg] : @"unknown."; \
            NSLogJSON(@{@"Event": @"Error", @"Code": @(err), @"Status": description}); \
            log_on_error(@"Error 0x%x: %@ " #call, err, description);               \
        }                                                                       \
    } while (false);



void disable_ssl(ServiceConnRef con)
{
    // MobileDevice links with SSL, so function will be available;
    typedef void (*SSL_free_t)(void*);
    static SSL_free_t SSL_free = NULL;
    if (SSL_free == NULL)
    {
        SSL_free = (SSL_free_t)dlsym(RTLD_DEFAULT, "SSL_free");
    }

    SSL_free(con->sslContext);
    con->sslContext = NULL;
}

void log_on_error(NSString* format, ...)
{
    va_list valist;
    va_start(valist, format);
    NSString* str = [[[NSString alloc] initWithFormat:format arguments:valist] autorelease];
    va_end(valist);

    if (!_json_output) {
        NSLog(@"[ !! ] %@", str);
    }
}


void on_error(NSString* format, ...)
{
    va_list valist;
    va_start(valist, format);
    NSString* str = [[[NSString alloc] initWithFormat:format arguments:valist] autorelease];
    va_end(valist);

    if (!_json_output) {
        NSLog(@"[ !! ] %@", str);
    }

    exit(exitcode_error);
}

// Print error message getting last errno and exit
void on_sys_error(NSString* format, ...) {
    const char* errstr = strerror(errno);

    va_list valist;
    va_start(valist, format);
    NSString* str = [[[NSString alloc] initWithFormat:format arguments:valist] autorelease];
    va_end(valist);

    on_error(@"%@ : %@", str, [NSString stringWithUTF8String:errstr]);
}

void __NSLogOut(NSString* format, va_list valist) {
    NSString* str = [[[NSString alloc] initWithFormat:format arguments:valist] autorelease];
    [[str stringByAppendingString:@"\n"] writeToFile:@"/dev/stdout" atomically:NO encoding:NSUTF8StringEncoding error:nil];
}

void NSLogOut(NSString* format, ...) {
    if (!_json_output) {
        va_list valist;
        va_start(valist, format);
        __NSLogOut(format, valist);
        va_end(valist);
    }
}

void NSLogVerbose(NSString* format, ...) {
    if (verbose && !_json_output) {
        va_list valist;
        va_start(valist, format);
        __NSLogOut(format, valist);
        va_end(valist);
    }
}

void NSLogJSON(NSDictionary* jsonDict) {
    if (_json_output) {
        NSError *error;
        NSData *data = [NSJSONSerialization dataWithJSONObject:jsonDict
                                                           options:NSJSONWritingPrettyPrinted
                                                             error:&error];
        if (data) {
            NSString *jsonString = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
            [jsonString writeToFile:@"/dev/stdout" atomically:NO encoding:NSUTF8StringEncoding error:nil];
            [jsonString release];
        } else {
            [@"{\"JSONError\": \"JSON error\"}" writeToFile:@"/dev/stdout" atomically:NO encoding:NSUTF8StringEncoding error:nil];
        }
    }
}

uint64_t get_current_time_in_milliseconds() {
    return clock_gettime_nsec_np(CLOCK_REALTIME) / (1000 * 1000);
}

BOOL mkdirp(NSString* path) {
    NSError* error = nil;
    BOOL success = [[NSFileManager defaultManager] createDirectoryAtPath:path
                                             withIntermediateDirectories:YES
                                                              attributes:nil
                                                                   error:&error];
    return success;
}

Boolean path_exists(CFTypeRef path) {
    if (CFGetTypeID(path) == CFStringGetTypeID()) {
        CFURLRef url = CFURLCreateWithFileSystemPath(NULL, path, kCFURLPOSIXPathStyle, true);
        Boolean result = CFURLResourceIsReachable(url, NULL);
        CFRelease(url);
        return result;
    } else if (CFGetTypeID(path) == CFURLGetTypeID()) {
        return CFURLResourceIsReachable(path, NULL);
    } else {
        return false;
    }
}

CFStringRef copy_find_path(CFStringRef rootPath, CFStringRef namePattern) {
    FILE *fpipe = NULL;
    CFStringRef cf_command;

    if( !path_exists(rootPath) )
        return NULL;

    if (faster_path_search) {
        CFIndex maxdepth = 1;
        CFArrayRef findPathSlash = CFStringCreateArrayWithFindResults(NULL, namePattern, CFSTR("/"), CFRangeMake(0, CFStringGetLength(namePattern)), 0);
        if (findPathSlash != NULL) {
            maxdepth = CFArrayGetCount(findPathSlash) + 1;
            CFRelease(findPathSlash);
        }

        cf_command = CFStringCreateWithFormat(NULL, NULL, CFSTR("find '%@' -path '%@/%@' -maxdepth %ld 2>/dev/null | sort | tail -n 1"), rootPath, rootPath, namePattern, maxdepth);
    }
    else {
        if (CFStringFind(namePattern, CFSTR("*"), 0).location == kCFNotFound) {
            //No wildcards. Let's speed up the search
            CFStringRef path = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@/%@"), rootPath, namePattern);
            
            if( path_exists(path) )
                return path;
            
            CFRelease(path);
            return NULL;
        }
        
        if (CFStringFind(namePattern, CFSTR("/"), 0).location == kCFNotFound) {
            cf_command = CFStringCreateWithFormat(NULL, NULL, CFSTR("find '%@' -name '%@' -maxdepth 1 2>/dev/null | sort | tail -n 1"), rootPath, namePattern);
        } else {
            cf_command = CFStringCreateWithFormat(NULL, NULL, CFSTR("find '%@' -path '%@/%@' 2>/dev/null | sort | tail -n 1"), rootPath, rootPath, namePattern);
        }
    }

    char command[1024] = { '\0' };
    CFStringGetCString(cf_command, command, sizeof(command), kCFStringEncodingUTF8);
    CFRelease(cf_command);

    if (!(fpipe = (FILE *)popen(command, "r")))
        on_sys_error(@"Error encountered while opening pipe");

    char buffer[256] = { '\0' };

    fgets(buffer, sizeof(buffer), fpipe);
    pclose(fpipe);

    strtok(buffer, "\n");
    
    CFStringRef path = CFStringCreateWithCString(NULL, buffer, kCFStringEncodingUTF8);
        
    if( CFStringGetLength(path) > 0 && path_exists(path) )
        return path;

    CFRelease(path);
    return NULL;
}

CFStringRef copy_xcode_dev_path(void) {
    static char xcode_dev_path[256] = { '\0' };
    if (strlen(xcode_dev_path) == 0) {
        const char* env_dev_path = getenv("DEVELOPER_DIR");
        
        if (env_dev_path && strlen(env_dev_path) > 0) {
            strcpy(xcode_dev_path, env_dev_path);
            // DEVELOPER_DIR should refer to Xcode.app/Contents/Developer, but
            // xcode-select and friends have an extension to fix the path, if it points to Xcode.app/.
            static char dev_subdir[256] = { '\0' };
            strcat(strcat(dev_subdir, env_dev_path), "/Contents/Developer");
            struct stat sb;
            if (stat(dev_subdir, &sb) == 0)
            {
                strcpy(xcode_dev_path, dev_subdir);
            }
        } else {
            FILE *fpipe = NULL;
            char *command = "xcode-select -print-path";

            if (!(fpipe = (FILE *)popen(command, "r")))
                on_sys_error(@"Error encountered while opening pipe");

            char buffer[256] = { '\0' };

            fgets(buffer, sizeof(buffer), fpipe);
            pclose(fpipe);

            strtok(buffer, "\n");
            strcpy(xcode_dev_path, buffer);
        }
        NSLogVerbose(@"Found Xcode developer dir %s", xcode_dev_path);
    }
    return CFStringCreateWithCString(NULL, xcode_dev_path, kCFStringEncodingUTF8);
}

const char *get_home(void) {
    const char* home = getenv("HOME");
    if (!home) {
        struct passwd *pwd = getpwuid(getuid());
        home = pwd->pw_dir;
    }
    return home;
}

CFStringRef copy_xcode_path_for_impl(CFStringRef rootPath, CFStringRef subPath, CFStringRef search) {
    CFStringRef searchPath = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@/%@"), rootPath, subPath );
    CFStringRef res = copy_find_path(searchPath, search);
    CFRelease(searchPath);
    return res;
}

CFStringRef copy_xcode_path_for(CFStringRef subPath, CFStringRef search) {
    CFStringRef xcodeDevPath = copy_xcode_dev_path();
    CFStringRef defaultXcodeDevPath = CFSTR("/Applications/Xcode.app/Contents/Developer");
    CFStringRef path = NULL;
    const char* home = get_home();
    
    // Try using xcode-select --print-path
    path = copy_xcode_path_for_impl(xcodeDevPath, subPath, search);
    
    // If not look in the default xcode location (xcode-select is sometimes wrong)
    if (path == NULL && CFStringCompare(xcodeDevPath, defaultXcodeDevPath, 0) != kCFCompareEqualTo )
        path = copy_xcode_path_for_impl(defaultXcodeDevPath, subPath, search);

    // If not look in the users home directory, Xcode can store device support stuff there
    if (path == NULL) {
        CFRelease(xcodeDevPath);
        xcodeDevPath = CFStringCreateWithFormat(NULL, NULL, CFSTR("%s/Library/Developer/Xcode"), home );
        path = copy_xcode_path_for_impl(xcodeDevPath, subPath, search);
    }

    CFRelease(xcodeDevPath);
    
    return path;
}

device_desc get_device_desc(CFStringRef model) {
    if (model != NULL) {
        size_t sz = sizeof(device_db) / sizeof(device_desc);
        for (size_t i = 0; i < sz; i ++) {
            if (CFStringCompare(model, device_db[i].model, kCFCompareNonliteral | kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
                return device_db[i];
            }
        }
    }
    
    device_desc res = device_db[UNKNOWN_DEVICE_IDX];
    
    res.model = model;
    res.name = model;
    
    return res;
}

bool is_usb_device(const AMDeviceRef device) {
  return AMDeviceGetInterfaceType(device) == 1;
}

void connect_and_start_session(AMDeviceRef device) {
    AMDeviceConnect(device);
    assert(AMDeviceIsPaired(device));
    check_error(AMDeviceValidatePairing(device));
    check_error(AMDeviceStartSession(device));
}

CFStringRef get_device_full_name(const AMDeviceRef device) {
    CFStringRef full_name = NULL,
                device_udid = AMDeviceCopyDeviceIdentifier(device),
                device_name = NULL,
                model_name = NULL,
                sdk_name = NULL,
                arch_name = NULL,
                product_version = NULL,
                build_version = NULL;

    AMDeviceConnect(device);

    device_name = AMDeviceCopyValue(device, 0, CFSTR("DeviceName"));

    // Please ensure that device is connected or the name will be unknown
    CFStringRef model = AMDeviceCopyValue(device, 0, CFSTR("HardwareModel"));
    device_desc dev;
    if (model != NULL) {
        dev = get_device_desc(model);
    } else {
        dev= device_db[UNKNOWN_DEVICE_IDX];
        model = dev.model;
    }
    model_name = dev.name;
    sdk_name = dev.sdk;
    arch_name = dev.arch;
    product_version = AMDeviceCopyValue(device, 0, CFSTR("ProductVersion"));
    build_version = AMDeviceCopyValue(device, 0, CFSTR("BuildVersion"));

    NSLogVerbose(@"Hardware Model: %@", model);
    NSLogVerbose(@"Device Name: %@", device_name);
    NSLogVerbose(@"Model Name: %@", model_name);
    NSLogVerbose(@"SDK Name: %@", sdk_name);
    NSLogVerbose(@"Architecture Name: %@", arch_name);
    NSLogVerbose(@"Product Version: %@", product_version);
    NSLogVerbose(@"Build Version: %@", build_version);
    if (build_version == 0)
        build_version = CFStringCreateWithCString(NULL, "", kCFStringEncodingUTF8);

    if (device_name != NULL) {
        full_name = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ (%@, %@, %@, %@, %@, %@) a.k.a. '%@'"), device_udid, model, model_name, sdk_name, arch_name, product_version, build_version, device_name);
    } else {
        full_name = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ (%@, %@, %@, %@, %@, %@)"), device_udid, model, model_name, sdk_name, arch_name, product_version, build_version);
    }

    AMDeviceDisconnect(device);

    if(device_udid != NULL)
        CFRelease(device_udid);
    if(device_name != NULL)
        CFRelease(device_name);
    if(model != NULL)
        CFRelease(model);
    if(model_name != NULL && model_name != model)
        CFRelease(model_name);
    if(product_version)
        CFRelease(product_version);
    if(build_version)
        CFRelease(build_version);

    return CFAutorelease(full_name);
}

NSDictionary* get_device_json_dict(const AMDeviceRef device) {
    NSMutableDictionary *json_dict = [NSMutableDictionary new];
    is_usb_device(device) ? AMDeviceConnect(device) : connect_and_start_session(device);
    
    CFStringRef device_udid = AMDeviceCopyDeviceIdentifier(device);
    if (device_udid) {
        [json_dict setValue:(__bridge NSString *)device_udid forKey:@"DeviceIdentifier"];
        CFRelease(device_udid);
    }
    
    CFStringRef device_hardware_model = AMDeviceCopyValue(device, 0, CFSTR("HardwareModel"));
    if (device_hardware_model) {
        [json_dict setValue:(NSString*)device_hardware_model forKey:@"HardwareModel"];
        size_t device_db_length = sizeof(device_db) / sizeof(device_desc);
        for (size_t i = 0; i < device_db_length; i ++) {
            if (CFStringCompare(device_hardware_model, device_db[i].model, kCFCompareNonliteral | kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
                device_desc dev = device_db[i];
                [json_dict setValue:(__bridge NSString *)dev.name forKey:@"modelName"];
                [json_dict setValue:(__bridge NSString *)dev.sdk forKey:@"modelSDK"];
                [json_dict setValue:(__bridge NSString *)dev.arch forKey:@"modelArch"];
                break;
            }
        }
        CFRelease(device_hardware_model);
    }
    
    for (NSString *deviceValue in @[@"DeviceName",
                                    @"BuildVersion",
                                    @"DeviceClass",
                                    @"ProductType",
                                    @"ProductVersion"]) {
        CFStringRef cf_value = AMDeviceCopyValue(device, 0, (__bridge CFStringRef)deviceValue);
        if (cf_value) {
            [json_dict setValue:(__bridge NSString *)cf_value forKey:deviceValue];
            CFRelease(cf_value);
        }
    }
    
    AMDeviceDisconnect(device);

    return CFAutorelease(json_dict);
}

int get_companion_interface_type(AMDeviceRef device)
{
    assert(AMDeviceGetInterfaceType(device) == 3);
    AMDeviceRef companion = AMDeviceCopyPairedCompanion(device);
    int type = AMDeviceGetInterfaceType(companion);
    AMDeviceRelease(companion);
    return type;
}

CFStringRef get_device_interface_name(const AMDeviceRef device) {
    // AMDeviceGetInterfaceType(device) 0=Unknown, 1 = Direct/USB, 2 = Indirect/WIFI, 3 = Companion proxy
    switch(AMDeviceGetInterfaceType(device)) {
        case 1:
            return CFSTR("USB");
        case 2:
            return CFSTR("WIFI");
        case 3:
        {
            if (get_companion_interface_type(device) == 1)
            {
                return CFSTR("USB Companion proxy");
            }
            else
            {
                return CFSTR("WIFI Companion proxy");
            }
        }
        default:
            return CFSTR("Unknown Connection");
    }
}

CFMutableArrayRef copy_device_product_version_parts(AMDeviceRef device) {
    CFStringRef version = AMDeviceCopyValue(device, 0, CFSTR("ProductVersion"));
    CFArrayRef parts = CFStringCreateArrayBySeparatingStrings(NULL, version, CFSTR("."));
    CFMutableArrayRef result = CFArrayCreateMutableCopy(NULL, CFArrayGetCount(parts), parts);
    CFRelease(version);
    CFRelease(parts);
    return result;
}

CFStringRef copy_device_support_path(AMDeviceRef device, CFStringRef suffix) {
    time_t startTime, endTime;
    time( &startTime );

    CFStringRef version = NULL;
    CFStringRef build = AMDeviceCopyValue(device, 0, CFSTR("BuildVersion"));
    CFStringRef deviceClass = AMDeviceCopyValue(device, 0, CFSTR("DeviceClass"));
    CFStringRef deviceModel = AMDeviceCopyValue(device, 0, CFSTR("HardwareModel"));
    CFStringRef productType = AMDeviceCopyValue(device, 0, CFSTR("ProductType"));
    CFStringRef deviceArch = NULL;
    CFStringRef path = NULL;

    device_desc dev;
    if (deviceModel != NULL) {
        dev = get_device_desc(deviceModel);
        deviceArch = dev.arch;
    }

    CFMutableArrayRef version_parts = copy_device_product_version_parts(device);

    NSLogVerbose(@"Device Class: %@", deviceClass);
    NSLogVerbose(@"build: %@", build);

    CFStringRef deviceClassPath[2];

    if (CFStringCompare(CFSTR("AppleTV"), deviceClass, 0) == kCFCompareEqualTo) {
      deviceClassPath[0] = CFSTR("Platforms/AppleTVOS.platform/DeviceSupport");
      deviceClassPath[1] = CFSTR("tvOS DeviceSupport");
    }
    else if (CFStringCompare(CFSTR("Watch"), deviceClass, 0) == kCFCompareEqualTo) {
      deviceClassPath[0] = CFSTR("Platforms/WatchOS.platform/DeviceSupport");
      deviceClassPath[1] = CFSTR("watchOS DeviceSupport");
    }
    else {
      deviceClassPath[0] = CFSTR("Platforms/iPhoneOS.platform/DeviceSupport");
      deviceClassPath[1] = CFSTR("iOS DeviceSupport");
    }

    CFMutableArrayRef string_allocations = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);
    while (CFArrayGetCount(version_parts) > 0) {
        version = CFStringCreateByCombiningStrings(NULL, version_parts, CFSTR("."));
        NSLogVerbose(@"version: %@", version);

        for( int i = 0; i < 2; ++i ) {
            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ (%@) %@/%@"), version, build, deviceArch, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }

            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ (%@)/%@"), version, build, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }

            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ (*)/%@"), version, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }

            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@/%@"), version, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }

            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@.*/%@"), version, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }
            if (path == NULL) {
                CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@ %@ (%@)/%@"), productType, version, build, suffix);
                path = copy_xcode_path_for(deviceClassPath[i], search);
                CFRelease(search);
            }
        }

        CFRelease(version);
        if (path != NULL) {
            break;
        }

        // Not all iOS versions have a dedicated developer disk image. Xcode 13.4.1 supports
        // iOS up to 15.5 but does not include developer disk images for 15.1 or 15.3
        // despite being able to deploy to them. For this reason, this logic looks for previous
        // minor versions if it doesn't find an exact match. In the case where the disk image
        // from a previous minor version is not compatible, deployment will fail with
        // kAMDInvalidServiceError.
        CFStringRef previous_minor_version = NULL;
        if (CFEqual(CFSTR("DeveloperDiskImage.dmg"), suffix) &&
            CFArrayGetCount(version_parts) == 2) {
            int minor_version = CFStringGetIntValue(CFArrayGetValueAtIndex(version_parts, 1));
            if (minor_version > 0) {
                previous_minor_version = CFStringCreateWithFormat(kCFAllocatorDefault, NULL,
                                                                  CFSTR("%d"), minor_version - 1);
                CFArrayAppendValue(string_allocations, previous_minor_version);
            }
        }
        CFArrayRemoveValueAtIndex(version_parts, CFArrayGetCount(version_parts) - 1);
        if (previous_minor_version) {
            CFArrayAppendValue(version_parts, previous_minor_version);
        }
    }

    for (int i = 0; i < CFArrayGetCount(string_allocations); i++) {
        CFRelease(CFArrayGetValueAtIndex(string_allocations, i));
    }
    CFRelease(string_allocations);

    for( int i = 0; i < 2; ++i ) {
        if (path == NULL) {
            CFStringRef search = CFStringCreateWithFormat(NULL, NULL, CFSTR("Latest/%@"), suffix);
            path = copy_xcode_path_for(deviceClassPath[i], search);
            CFRelease(search);
        }
    }

    CFRelease(version_parts);
    CFRelease(build);
    CFRelease(deviceClass);
    CFRelease(productType);
    if (deviceModel != NULL) {
        CFRelease(deviceModel);
    }
    if (path == NULL) {
      NSString *msg = [NSString stringWithFormat:@"Unable to locate DeviceSupport directory with suffix '%@'. This probably means you don't have Xcode installed, you will need to launch the app manually and logging output will not be shown!", suffix];
        NSLogJSON(@{
          @"Event": @"DeviceSupportError",
          @"Status": msg,
        });
        on_error(msg);
    }
    
    time( &endTime );
    NSLogVerbose(@"DeviceSupport directory '%@' was located. It took %.2f seconds", path, difftime(endTime,startTime));
    
    return path;
}

void mount_callback(CFDictionaryRef dict, int arg) {
    CFStringRef status = CFDictionaryGetValue(dict, CFSTR("Status"));

    if (CFEqual(status, CFSTR("LookingUpImage"))) {
        NSLogOut(@"[  0%%] Looking up developer disk image");
    } else if (CFEqual(status, CFSTR("CopyingImage"))) {
        NSLogOut(@"[ 30%%] Copying DeveloperDiskImage.dmg to device");
    } else if (CFEqual(status, CFSTR("MountingImage"))) {
        NSLogOut(@"[ 90%%] Mounting developer disk image");
    }
}

void mount_developer_image(AMDeviceRef device) {
    CFStringRef image_path = copy_device_support_path(device, CFSTR("DeveloperDiskImage.dmg"));
    CFStringRef sig_path = CFStringCreateWithFormat(NULL, NULL, CFSTR("%@.signature"), image_path);

    NSLogVerbose(@"Developer disk image: %@", image_path);

    FILE* sig = fopen(CFStringGetCStringPtr(sig_path, kCFStringEncodingMacRoman), "rb");
    size_t buf_size = 128;
    void *sig_buf = malloc(buf_size);
    size_t bytes_read = fread(sig_buf, 1, buf_size, sig);
    if (bytes_read != buf_size) {
      on_sys_error(@"fread read %d bytes but expected %d bytes.", bytes_read, buf_size);
    }
    fclose(sig);
    CFDataRef sig_data = CFDataCreateWithBytesNoCopy(NULL, sig_buf, buf_size, NULL);
    CFRelease(sig_path);

    CFTypeRef keys[] = { CFSTR("ImageSignature"), CFSTR("ImageType") };
    CFTypeRef values[] = { sig_data, CFSTR("Developer") };
    CFDictionaryRef options = CFDictionaryCreate(NULL, (const void **)&keys, (const void **)&values, 2, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFRelease(sig_data);

    unsigned int result = (unsigned int)AMDeviceMountImage(device, image_path, options, &mount_callback, 0);
    if (result == 0) {
        NSLogOut(@"[ 95%%] Developer disk image mounted successfully");
    } else if (result == 0xe8000076 /* already mounted */) {
        NSLogOut(@"[ 95%%] Developer disk image already mounted");
    } else {
        if (result != 0) {
            const char* msg = get_error_message(result);
            NSString *description = @"unknown.";
            if (msg) {
                description = [NSString stringWithUTF8String:msg];
                NSLogOut(@"Error: %@", description);
            }
            NSLogJSON(@{@"Event": @"Error",
                        @"Code": @(result),
                        @"Status": description});
        }
        
        on_error(@"Unable to mount developer disk image. (%x)", result);
    }
  
    CFStringRef symbols_path = copy_device_support_path(device, CFSTR("Symbols"));
    if (symbols_path != NULL)
    {
        NSLogOut(@"Symbol Path: %@", symbols_path);
        NSLogJSON(@{@"Event": @"MountDeveloperImage",
                    @"SymbolsPath": (__bridge NSString *)symbols_path
                    });
        CFRelease(symbols_path);
    }
  
    CFRelease(image_path);
    CFRelease(options);
}

mach_error_t transfer_callback(CFDictionaryRef dict, int arg) {
    if (CFDictionaryGetValue(dict, CFSTR("Error"))) {
        return 0;
    }
    int percent;
    CFStringRef status = CFDictionaryGetValue(dict, CFSTR("Status"));
    CFNumberGetValue(CFDictionaryGetValue(dict, CFSTR("PercentComplete")), kCFNumberSInt32Type, &percent);

    if (CFEqual(status, CFSTR("CopyingFile"))) {
        static CFStringRef last_path = NULL;
        static int last_overall_percent = -1;

        CFStringRef path = CFDictionaryGetValue(dict, CFSTR("Path"));
        int overall_percent = percent / 2;

        if ((last_path == NULL || !CFEqual(path, last_path) || last_overall_percent != overall_percent) && !CFStringHasSuffix(path, CFSTR(".ipa"))) {
            NSLogOut(@"[%3d%%] Copying %@ to device", overall_percent, path);
            NSLogJSON(@{@"Event": @"BundleCopy",
                        @"OverallPercent": @(overall_percent),
                        @"Percent": @(percent),
                        @"Path": (__bridge NSString *)path
                        });
        }

        last_overall_percent = overall_percent;

        if (last_path != NULL) {
            CFRelease(last_path);
        }
        last_path = CFStringCreateCopy(NULL, path);
    }

    return 0;
}

mach_error_t install_callback(CFDictionaryRef dict, int arg) {
    if (CFDictionaryGetValue(dict, CFSTR("Error"))) {
        return 0;
    }
    int percent;
    CFStringRef status = CFDictionaryGetValue(dict, CFSTR("Status"));
    CFNumberGetValue(CFDictionaryGetValue(dict, CFSTR("PercentComplete")), kCFNumberSInt32Type, &percent);

    int overall_percent = (percent / 2) + 50;
    NSLogOut(@"[%3d%%] %@", overall_percent, status);
    NSLogJSON(@{@"Event": @"BundleInstall",
                @"OverallPercent": @(overall_percent),
                @"Percent": @(percent),
                @"Status": (__bridge NSString *)status
                });
    return 0;
}

// During standard installation transferring and installation takes place
// in distinct function that can be passed distinct callbacks. Incremental
// installation performs both transfer and installation in a single function so
// use this callback to determine which step is occuring and call the proper
// callback.
mach_error_t incremental_install_callback(CFDictionaryRef dict, int arg) {
    if (CFDictionaryGetValue(dict, CFSTR("Error"))) {
        return 0;
    }
    CFStringRef status = CFDictionaryGetValue(dict, CFSTR("Status"));
    if (CFEqual(status, CFSTR("TransferringPackage"))) {
        int percent;
        CFNumberGetValue(CFDictionaryGetValue(dict, CFSTR("PercentComplete")), kCFNumberSInt32Type, &percent);
        int overall_percent = (percent / 2);
        NSLogOut(@"[%3d%%] %@", overall_percent, status);
        NSLogJSON(@{@"Event": @"TransferringPackage",
                    @"OverallPercent": @(overall_percent),
                    });
        return 0;
    } else if (CFEqual(status, CFSTR("CopyingFile"))) {
        return transfer_callback(dict, arg);
    } else {
        return install_callback(dict, arg);
    }
}

CFURLRef copy_device_app_url(AMDeviceRef device, CFStringRef identifier) {
    CFDictionaryRef result = nil;

    NSArray *a = [NSArray arrayWithObjects:
                  @"CFBundleIdentifier",            // absolute must
                  @"ApplicationDSID",
                  @"ApplicationType",
                  @"CFBundleExecutable",
                  @"CFBundleDisplayName",
                  @"CFBundleIconFile",
                  @"CFBundleName",
                  @"CFBundleShortVersionString",
                  @"CFBundleSupportedPlatforms",
                  @"CFBundleURLTypes",
                  @"CodeInfoIdentifier",
                  @"Container",
                  @"Entitlements",
                  @"HasSettingsBundle",
                  @"IsUpgradeable",
                  @"MinimumOSVersion",
                  @"Path",
                  @"SignerIdentity",
                  @"UIDeviceFamily",
                  @"UIFileSharingEnabled",
                  @"UIStatusBarHidden",
                  @"UISupportedInterfaceOrientations",
                  nil];

    NSDictionary *optionsDict = [NSDictionary dictionaryWithObject:a forKey:@"ReturnAttributes"];
    CFDictionaryRef options = (CFDictionaryRef)optionsDict;

    check_error(AMDeviceLookupApplications(device, options, &result));

    CFDictionaryRef app_dict = CFDictionaryGetValue(result, identifier);
    assert(app_dict != NULL);

    CFStringRef app_path = CFDictionaryGetValue(app_dict, CFSTR("Path"));
    assert(app_path != NULL);

    CFURLRef url = CFURLCreateWithFileSystemPath(NULL, app_path, kCFURLPOSIXPathStyle, true);
    CFRelease(result);
    return url;
}

CFStringRef copy_disk_app_identifier(CFURLRef disk_app_url) {
    CFURLRef plist_url = CFURLCreateCopyAppendingPathComponent(NULL, disk_app_url, CFSTR("Info.plist"), false);
    CFReadStreamRef plist_stream = CFReadStreamCreateWithFile(NULL, plist_url);
    if (!CFReadStreamOpen(plist_stream)) {
        on_error(@"Cannot read Info.plist file: %@", plist_url);
    }

    CFPropertyListRef plist = CFPropertyListCreateWithStream(NULL, plist_stream, 0, kCFPropertyListImmutable, NULL, NULL);
    CFStringRef bundle_identifier = CFRetain(CFDictionaryGetValue(plist, CFSTR("CFBundleIdentifier")));
    CFReadStreamClose(plist_stream);

    CFRelease(plist_url);
    CFRelease(plist_stream);
    CFRelease(plist);

    return bundle_identifier;
}

CFStringRef copy_modules_search_paths_pairs(CFStringRef symbols_path, CFStringRef disk_container, CFStringRef device_container_private, CFStringRef device_container_noprivate )
{
    CFMutableStringRef res = CFStringCreateMutable(kCFAllocatorDefault, 0);
    CFStringAppendFormat(res, NULL, CFSTR("/usr \"%@/usr\""), symbols_path);
    CFStringAppendFormat(res, NULL, CFSTR(" /System \"%@/System\""), symbols_path);
    CFStringAppendFormat(res, NULL, CFSTR(" \"%@\" \"%@\""), device_container_private, disk_container);
    CFStringAppendFormat(res, NULL, CFSTR(" \"%@\" \"%@\""), device_container_noprivate, disk_container);
    CFStringAppendFormat(res, NULL, CFSTR(" /Developer \"%@/Developer\""), symbols_path);
    
    return res;
}

CFStringRef get_device_platform(AMDeviceRef device)
{
    CFStringRef deviceClass = AMDeviceCopyValue(device, 0, CFSTR("DeviceClass"));
    CFStringRef platform;
    if (CFStringCompare(CFSTR("AppleTV"), deviceClass, 0) == kCFCompareEqualTo) {
        platform = CFSTR("tvos");
    }
    else if (CFStringCompare(CFSTR("Watch"), deviceClass, 0) == kCFCompareEqualTo) {
        platform = CFSTR("watchos");
    }
    else {
        platform = CFSTR("ios");
    }
    CFRelease(deviceClass);
    return platform;
}

void write_lldb_prep_cmds(AMDeviceRef device, CFURLRef disk_app_url) {
    CFStringRef symbols_path = copy_device_support_path(device, CFSTR("Symbols"));
    CFMutableStringRef cmds = CFStringCreateMutableCopy(NULL, 0, LLDB_PREP_CMDS);
    CFRange range = { 0, CFStringGetLength(cmds) };

    CFStringFindAndReplace(cmds, CFSTR("{platform}"), get_device_platform(device), range, 0);
    range.length = CFStringGetLength(cmds);

    CFStringFindAndReplace(cmds, CFSTR("{symbols_path}"), symbols_path, range, 0);
    range.length = CFStringGetLength(cmds);

    CFMutableStringRef pmodule = CFStringCreateMutableCopy(NULL, 0, (CFStringRef)LLDB_FRUITSTRAP_MODULE);

    CFRange rangeLLDB = { 0, CFStringGetLength(pmodule) };
    
    CFStringRef exitcode_app_crash_str = CFStringCreateWithFormat(NULL, NULL, CFSTR("%d"), exitcode_app_crash);
    CFStringFindAndReplace(pmodule, CFSTR("{exitcode_app_crash}"), exitcode_app_crash_str, rangeLLDB, 0);
    CFRelease(exitcode_app_crash_str);
    rangeLLDB.length = CFStringGetLength(pmodule);
    
    CFStringRef detect_deadlock_timeout_str = CFStringCreateWithFormat(NULL, NULL, CFSTR("%d"), _detectDeadlockTimeout);
    CFStringFindAndReplace(pmodule, CFSTR("{detect_deadlock_timeout}"), detect_deadlock_timeout_str, rangeLLDB, 0);
    CFRelease(detect_deadlock_timeout_str);
    rangeLLDB.length = CFStringGetLength(pmodule);

    if (args) {
        CFStringRef cf_args = CFStringCreateWithCString(NULL, args, kCFStringEncodingUTF8);
        CFStringFindAndReplace(cmds, CFSTR("{args}"), cf_args, range, 0);
        rangeLLDB.length = CFStringGetLength(pmodule);
        CFStringFindAndReplace(pmodule, CFSTR("{args}"), cf_args, rangeLLDB, 0);

        //printf("write_lldb_prep_cmds:args: [%s][%s]\n", CFStringGetCStringPtr (cmds,kCFStringEncodingMacRoman),
        //    CFStringGetCStringPtr(pmodule, kCFStringEncodingMacRoman));
        CFRelease(cf_args);
    } else {
        CFStringFindAndReplace(cmds, CFSTR("{args}"), CFSTR(""), range, 0);
        CFStringFindAndReplace(pmodule, CFSTR("{args}"), CFSTR(""), rangeLLDB, 0);
        //printf("write_lldb_prep_cmds: [%s][%s]\n", CFStringGetCStringPtr (cmds,kCFStringEncodingMacRoman),
        //    CFStringGetCStringPtr(pmodule, kCFStringEncodingMacRoman));
    }

    if (envs) {
        CFStringRef cf_envs = CFStringCreateWithCString(NULL, envs, kCFStringEncodingUTF8);
        CFStringFindAndReplace(cmds, CFSTR("{envs}"), cf_envs, range, 0);
        rangeLLDB.length = CFStringGetLength(pmodule);
        CFStringFindAndReplace(pmodule, CFSTR("{envs}"), cf_envs, rangeLLDB, 0);

        //printf("write_lldb_prep_cmds:envs: [%s][%s]\n", CFStringGetCStringPtr (cmds,kCFStringEncodingMacRoman),
        //    CFStringGetCStringPtr(pmodule, kCFStringEncodingMacRoman));
        CFRelease(cf_envs);
    } else {
        CFStringFindAndReplace(cmds, CFSTR("{envs}"), CFSTR(""), range, 0);
        CFStringFindAndReplace(pmodule, CFSTR("{envs}"), CFSTR(""), rangeLLDB, 0);
        //printf("write_lldb_prep_cmds: [%s][%s]\n", CFStringGetCStringPtr (cmds,kCFStringEncodingMacRoman),
        //    CFStringGetCStringPtr(pmodule, kCFStringEncodingMacRoman));
    }
    range.length = CFStringGetLength(cmds);

    CFStringRef bundle_identifier = copy_disk_app_identifier(disk_app_url);
    CFURLRef device_app_url = copy_device_app_url(device, bundle_identifier);
    CFRelease(bundle_identifier);
    CFStringRef device_app_path = CFURLCopyFileSystemPath(device_app_url, kCFURLPOSIXPathStyle);
    CFStringFindAndReplace(cmds, CFSTR("{device_app}"), device_app_path, range, 0);
    CFRelease(device_app_path);
    range.length = CFStringGetLength(cmds);

    CFStringRef disk_app_path = CFURLCopyFileSystemPath(disk_app_url, kCFURLPOSIXPathStyle);
    CFStringFindAndReplace(cmds, CFSTR("{disk_app}"), disk_app_path, range, 0);
    CFRelease(disk_app_path);
    range.length = CFStringGetLength(cmds);

    CFStringRef device_port = CFStringCreateWithFormat(NULL, NULL, CFSTR("%d"), port);
    CFStringFindAndReplace(cmds, CFSTR("{device_port}"), device_port, range, 0);
    CFRelease(device_port);
    range.length = CFStringGetLength(cmds);

    if (output_path) {
        CFStringRef output_path_str = CFStringCreateWithCString(NULL, output_path, kCFStringEncodingUTF8);
        CFStringFindAndReplace(cmds, CFSTR("{output_path}"), output_path_str, range, 0);
        CFRelease(output_path_str);
    } else {
        CFStringFindAndReplace(cmds, CFSTR("{output_path}"), CFSTR(""), range, 0);
    }
    range.length = CFStringGetLength(cmds);
    if (error_path) {
        CFStringRef error_path_str = CFStringCreateWithCString(NULL, error_path, kCFStringEncodingUTF8);
        CFStringFindAndReplace(cmds, CFSTR("{error_path}"), error_path_str, range, 0);
        CFRelease(error_path_str);
    } else {
        CFStringFindAndReplace(cmds, CFSTR("{error_path}"), CFSTR(""), range, 0);
    }
    range.length = CFStringGetLength(cmds);

    CFURLRef device_container_url = CFURLCreateCopyDeletingLastPathComponent(NULL, device_app_url);
    CFRelease(device_app_url);
    CFStringRef device_container_path = CFURLCopyFileSystemPath(device_container_url, kCFURLPOSIXPathStyle);
    CFRelease(device_container_url);
    CFMutableStringRef dcp_noprivate = CFStringCreateMutableCopy(NULL, 0, device_container_path);
    range.length = CFStringGetLength(dcp_noprivate);
    CFStringFindAndReplace(dcp_noprivate, CFSTR("/private/var/"), CFSTR("/var/"), range, 0);
    range.length = CFStringGetLength(cmds);
    CFStringFindAndReplace(cmds, CFSTR("{device_container}"), dcp_noprivate, range, 0);
    range.length = CFStringGetLength(cmds);

    CFURLRef disk_container_url = CFURLCreateCopyDeletingLastPathComponent(NULL, disk_app_url);
    CFStringRef disk_container_path = CFURLCopyFileSystemPath(disk_container_url, kCFURLPOSIXPathStyle);
    CFRelease(disk_container_url);
    CFStringFindAndReplace(cmds, CFSTR("{disk_container}"), disk_container_path, range, 0);
    range.length = CFStringGetLength(cmds);
    
    CFStringRef search_paths_pairs = copy_modules_search_paths_pairs(symbols_path, disk_container_path, device_container_path, dcp_noprivate);
    CFRelease(symbols_path);
    CFRelease(device_container_path);
    CFRelease(dcp_noprivate);
    CFRelease(disk_container_path);
    CFStringFindAndReplace(cmds, CFSTR("{modules_search_paths_pairs}"), search_paths_pairs, range, 0);
    range.length = CFStringGetLength(cmds);
    CFRelease(search_paths_pairs);
    
    NSString* python_file_path = [NSString stringWithFormat:@"/tmp/%@/fruitstrap_", tmpUUID];
    mkdirp(python_file_path);

    NSString* python_command = @"fruitstrap_";
    if(device_id != NULL) {
        python_file_path = [python_file_path stringByAppendingString:[[NSString stringWithUTF8String:device_id] stringByReplacingOccurrencesOfString:@"-" withString:@"_"]];
        python_command = [python_command stringByAppendingString:[[NSString stringWithUTF8String:device_id] stringByReplacingOccurrencesOfString:@"-" withString:@"_"]];
    }
    python_file_path = [python_file_path stringByAppendingString:@".py"];

    CFStringFindAndReplace(cmds, CFSTR("{python_command}"), (CFStringRef)python_command, range, 0);
    range.length = CFStringGetLength(cmds);
    CFStringFindAndReplace(cmds, CFSTR("{python_file_path}"), (CFStringRef)python_file_path, range, 0);
    range.length = CFStringGetLength(cmds);

    CFDataRef cmds_data = CFStringCreateExternalRepresentation(NULL, cmds, kCFStringEncodingUTF8, 0);
    NSString* prep_cmds_path = [NSString stringWithFormat:PREP_CMDS_PATH, tmpUUID];
    if(device_id != NULL) {
        prep_cmds_path = [prep_cmds_path stringByAppendingString:[[NSString stringWithUTF8String:device_id] stringByReplacingOccurrencesOfString:@"-" withString:@"_"]];
    }
    FILE *out = fopen([prep_cmds_path UTF8String], "w");
    fwrite(CFDataGetBytePtr(cmds_data), CFDataGetLength(cmds_data), 1, out);
    CFRelease(cmds_data);
    // Write additional commands based on mode we're running in
    const char* extra_cmds;
    if (!interactive)
    {
        if (justlaunch)
          extra_cmds = lldb_prep_noninteractive_justlaunch_cmds;
        else
          extra_cmds = lldb_prep_noninteractive_cmds;
    }
    else if (nostart)
        extra_cmds = lldb_prep_no_cmds;
    else
        extra_cmds = lldb_prep_interactive_cmds;
    fwrite(extra_cmds, strlen(extra_cmds), 1, out);
    if (custom_commands != nil)
    {
        const char * cmds = [custom_commands UTF8String];
        fwrite(cmds, 1, strlen(cmds), out);
    }
    fclose(out);


    out = fopen([python_file_path UTF8String], "w");
    CFDataRef pmodule_data = CFStringCreateExternalRepresentation(NULL, pmodule, kCFStringEncodingUTF8, 0);
    fwrite(CFDataGetBytePtr(pmodule_data), CFDataGetLength(pmodule_data), 1, out);
    CFRelease(pmodule_data);

    if (custom_script_path)
    {
        FILE * fh = fopen(custom_script_path, "r");
        if (fh == NULL)
        {
            on_error(@"Failed to open %s", custom_script_path);
        }
        fwrite("\n", 1, 1, out);
        char buffer[0x1000];
        size_t bytesRead;
        while ((bytesRead = fread(buffer, 1, sizeof(buffer), fh)) > 0)
        {
            fwrite(buffer, 1, bytesRead, out);
        }
        fclose(fh);
    }

    fclose(out);

    CFRelease(cmds);
    CFRelease(pmodule);
}

int kill_ptree(pid_t root, int signum);

const CFStringRef kDbgConnectionPropertyServiceConnection = CFSTR("service_connection");
const CFStringRef kDbgConnectionPropertyLLDBSocket = CFSTR("lldb_socket");
const CFStringRef kDbgConnectionPropertyLLDBSocketRunLoop = CFSTR("lldb_socket_runloop");
const CFStringRef kDbgConnectionPropertyServerSocket = CFSTR("server_socket");
const CFStringRef kDbgConnectionPropertyServerSocketRunLoop = CFSTR("server_socket_runloop");

CFSocketContext get_socket_context(CFNumberRef connection_id) {
    CFSocketContext context = { 0, (void*)connection_id, NULL, NULL, NULL };
    return context;
}

CFMutableDictionaryRef get_connection_properties(CFNumberRef connection_id) {
    // This is no-op if the key already exists
    CFDictionaryAddValue(debugserver_active_connections, connection_id, CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));

    return (CFMutableDictionaryRef)CFDictionaryGetValue(debugserver_active_connections, connection_id);
}

void close_connection(CFNumberRef connection_id) {
    CFMutableDictionaryRef connection_properties = get_connection_properties(connection_id);

    ServiceConnRef dbgServiceConnection = (ServiceConnRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyServiceConnection);
    AMDServiceConnectionInvalidate(dbgServiceConnection);
    CFRelease(dbgServiceConnection);

    CFSocketRef lldb_socket = (CFSocketRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyLLDBSocket);
    CFSocketInvalidate(lldb_socket);
    CFRelease(lldb_socket);

    CFRunLoopSourceRef lldb_socket_runloop = (CFRunLoopSourceRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyLLDBSocketRunLoop);
    CFRunLoopRemoveSource(CFRunLoopGetMain(), lldb_socket_runloop, kCFRunLoopCommonModes);
    CFRelease(lldb_socket_runloop);

    CFSocketRef server_socket = (CFSocketRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyServerSocket);
    CFSocketInvalidate(server_socket);
    CFRelease(server_socket);

    CFRunLoopSourceRef server_socket_runloop = (CFRunLoopSourceRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyServerSocketRunLoop);
    CFRunLoopRemoveSource(CFRunLoopGetMain(), server_socket_runloop, kCFRunLoopCommonModes);
    CFRelease(server_socket_runloop);

    CFDictionaryRemoveValue(debugserver_active_connections, connection_id);
}

void server_callback(CFSocketRef s, CFSocketCallBackType callbackType, CFDataRef address, const void *data, void *info) {
    CFNumberRef connection_id = (CFNumberRef)info;

    CFMutableDictionaryRef connection_properties = get_connection_properties(connection_id);

    ServiceConnRef dbgServiceConnection = (ServiceConnRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyServiceConnection);
    CFSocketRef lldb_socket = (CFSocketRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyLLDBSocket);

    char buffer[0x1000];
    int bytesRead;
    do {
        bytesRead = AMDServiceConnectionReceive(dbgServiceConnection, buffer, sizeof(buffer));
        if (bytesRead == 0)
        {
            // close the socket on which we've got end-of-file, the server_socket.
            close_connection(connection_id);
            return;
        }
        write(CFSocketGetNative(lldb_socket), buffer, bytesRead);
    }
    while (bytesRead == sizeof(buffer));
}

void lldb_callback(CFSocketRef s, CFSocketCallBackType callbackType, CFDataRef address, const void *data, void *info)
{
    CFNumberRef connection_id = (CFNumberRef)info;

    CFMutableDictionaryRef connection_properties = get_connection_properties(connection_id);

    ServiceConnRef dbgServiceConnection = (ServiceConnRef)CFDictionaryGetValue(connection_properties, kDbgConnectionPropertyServiceConnection);

    if (CFDataGetLength(data) == 0) {
        // close the socket on which we've got end-of-file, the lldb_socket.
        close_connection(connection_id);
        return;
    }
    int __unused sent = AMDServiceConnectionSend(dbgServiceConnection, CFDataGetBytePtr(data),  CFDataGetLength (data));
    assert (CFDataGetLength (data) == sent);
}

ServiceConnRef start_remote_debug_server(AMDeviceRef device) {
    ServiceConnRef dbgServiceConnection = NULL;
    CFStringRef serviceName = CFSTR("com.apple.debugserver");
    CFStringRef keys[] = { CFSTR("MinIPhoneVersion"), CFSTR("MinAppleTVVersion"), CFSTR("MinWatchVersion") };
    CFStringRef values[] = { CFSTR("14.0"), CFSTR("14.0"), CFSTR("7.0") }; // Not sure about older watchOS versions
    CFDictionaryRef version = CFDictionaryCreate(NULL, (const void **)&keys, (const void **)&values, 3, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

    bool useSecureProxy = AMDeviceIsAtLeastVersionOnPlatform(device, version);
    if (useSecureProxy)
    {
        serviceName = CFSTR("com.apple.debugserver.DVTSecureSocketProxy");
    }

    int start_err = AMDeviceSecureStartService(device, serviceName, NULL, &dbgServiceConnection);
    if (start_err != 0)
    {
        // After we mount the image, iOS needs to scan the image to register new services.
        // If we ask to start the service before it is found by ios, we will get 0xe8000022.
        // In other cases, it's been observed, that device may loose connection here (0xe800002d).
        // Luckly, we can just restart the connection and continue.
        // In other cases we just error out.
        NSLogOut(@"Failed to start debugserver: %x %s", start_err, get_error_message(start_err));
        switch(start_err)
        {
            case 0xe8000022:
                NSLogOut(@"Waiting for the device to scan mounted image");
                sleep(1);
                break;
            case 0x800002d:
                NSLogOut(@"Reconnecting to device");
                // We dont call AMDeviceStopSession as we cannot send any messages anymore
                check_error(AMDeviceDisconnect(device));
                connect_and_start_session(device);
                break;
            default:
                check_error(start_err);
        }
        check_error(AMDeviceSecureStartService(device, serviceName, NULL, &dbgServiceConnection));
    }
    assert(dbgServiceConnection != NULL);

    if (!useSecureProxy)
    {
        disable_ssl(dbgServiceConnection);
    }

    return dbgServiceConnection;
}

void create_remote_debug_server_socket(CFNumberRef connection_id, AMDeviceRef device) {
    CFMutableDictionaryRef connection_properties = get_connection_properties(connection_id);

    ServiceConnRef dbgServiceConnection = start_remote_debug_server(device);
    CFDictionaryAddValue(connection_properties, kDbgConnectionPropertyServiceConnection, dbgServiceConnection);

    /*
     * The debugserver connection is through a fd handle, while lldb requires a host/port to connect, so create an intermediate
     * socket to transfer data.
     */
    CFSocketContext context = get_socket_context(connection_id);
    CFSocketRef server_socket = CFSocketCreateWithNative (NULL, AMDServiceConnectionGetSocket(dbgServiceConnection), kCFSocketReadCallBack, &server_callback, &context);
    CFDictionaryAddValue(connection_properties, kDbgConnectionPropertyServerSocket, server_socket);

    CFRunLoopSourceRef server_socket_runloop = CFSocketCreateRunLoopSource(NULL, server_socket, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), server_socket_runloop, kCFRunLoopCommonModes);
    CFDictionaryAddValue(connection_properties, kDbgConnectionPropertyServerSocketRunLoop, server_socket_runloop);
}

void create_local_lldb_socket(CFNumberRef connection_id, CFSocketNativeHandle socket) {
    CFMutableDictionaryRef connection_properties = get_connection_properties(connection_id);

    CFSocketContext context = get_socket_context(connection_id);
    CFSocketRef lldb_socket  = CFSocketCreateWithNative(NULL, socket, kCFSocketDataCallBack, &lldb_callback, &context);
    CFDictionaryAddValue(connection_properties, kDbgConnectionPropertyLLDBSocket, lldb_socket);

    int flag = 1;
    int res = setsockopt(socket, IPPROTO_TCP, TCP_NODELAY, (char *) &flag, sizeof(flag));
    if (res == -1) {
      on_sys_error(@"Setting socket option failed.");
    }
    CFRunLoopSourceRef lldb_socket_runloop = CFSocketCreateRunLoopSource(NULL, lldb_socket, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), lldb_socket_runloop, kCFRunLoopCommonModes);
    CFDictionaryAddValue(connection_properties, kDbgConnectionPropertyLLDBSocketRunLoop, lldb_socket_runloop);
}

void fdvendor_callback(CFSocketRef s, CFSocketCallBackType callbackType, CFDataRef address, const void *data, void *info) {
    static int next_connection_id = 0;
    CFNumberRef connection_id = CFAutorelease(CFNumberCreate(NULL, kCFNumberIntType, &next_connection_id));

    assert (callbackType == kCFSocketAcceptCallBack);

    if (debugserver_only) {
        // In case of server mode, we start the debug server connection every time we accept a connection
        create_remote_debug_server_socket(connection_id, (AMDeviceRef)info);
        ++next_connection_id;
    }

    CFSocketNativeHandle socket = (CFSocketNativeHandle)(*((CFSocketNativeHandle *)data));
    create_local_lldb_socket(connection_id, socket);

    if (!debugserver_only) {
        // Stop listening after first connection in case not in server mode
        CFSocketInvalidate(s);
        CFRelease(s);
    }
}

void start_debug_server_multiplexer(AMDeviceRef device) {
    debugserver_active_connections = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

    struct sockaddr_in addr4;
    memset(&addr4, 0, sizeof(addr4));
    addr4.sin_len = sizeof(addr4);
    addr4.sin_family = AF_INET;
    addr4.sin_port = htons(port);
    addr4.sin_addr.s_addr = htonl(INADDR_LOOPBACK);

    CFSocketContext context = { 0, device, NULL, NULL, NULL };
    CFSocketRef fdvendor = CFSocketCreate(NULL, PF_INET, 0, 0, kCFSocketAcceptCallBack, &fdvendor_callback, &context);

    if (port) {
        int yes = 1;
        setsockopt(CFSocketGetNative(fdvendor), SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
    }

    CFDataRef address_data = CFDataCreate(NULL, (const UInt8 *)&addr4, sizeof(addr4));

    CFSocketSetAddress(fdvendor, address_data);
    CFRelease(address_data);
    socklen_t addrlen = sizeof(addr4);
    int res = getsockname(CFSocketGetNative(fdvendor),(struct sockaddr *)&addr4,&addrlen);
    if (res == -1) {
      on_sys_error(@"Getting socket name failed.");
    }
    port = ntohs(addr4.sin_port);

    if (fdvendor_runloop) {
        CFRelease(fdvendor_runloop);
    }
    fdvendor_runloop = CFSocketCreateRunLoopSource(NULL, fdvendor, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), fdvendor_runloop, kCFRunLoopCommonModes);
}

void kill_ptree_inner(pid_t root, int signum, struct kinfo_proc *kp, int kp_len) {
    int i;
    for (i = 0; i < kp_len; i++) {
        if (kp[i].kp_eproc.e_ppid == root) {
            kill_ptree_inner(kp[i].kp_proc.p_pid, signum, kp, kp_len);
        }
    }
    if (root != getpid()) {
        kill(root, signum);
    }
}

int kill_ptree(pid_t root, int signum) {
    int mib[3];
    size_t len;
    mib[0] = CTL_KERN;
    mib[1] = KERN_PROC;
    mib[2] = KERN_PROC_ALL;
    if (sysctl(mib, 3, NULL, &len, NULL, 0) == -1) {
        return -1;
    }

    struct kinfo_proc *kp = calloc(1, len);
    if (!kp) {
        return -1;
    }

    if (sysctl(mib, 3, kp, &len, NULL, 0) == -1) {
        free(kp);
        return -1;
    }

    kill_ptree_inner(root, signum, kp, (int)(len / sizeof(struct kinfo_proc)));

    free(kp);
    return 0;
}

void killed(int signum) {
    // SIGKILL needed to kill lldb, probably a better way to do this.
    kill(0, SIGKILL);
    _exit(0);
}

void lldb_finished_handler(int signum)
{
    int status = 0;
    if (waitpid(child, &status, 0) == -1)
        perror("waitpid failed");
    _exit(WEXITSTATUS(status));
}

pid_t bring_process_to_foreground(void) {
    pid_t fgpid = tcgetpgrp(STDIN_FILENO);
    if (setpgid(0, 0) == -1)
        perror("setpgid failed");

    signal(SIGTTOU, SIG_IGN);
    if (tcsetpgrp(STDIN_FILENO, getpid()) == -1)
        perror("tcsetpgrp failed");
    signal(SIGTTOU, SIG_DFL);
    return fgpid;
}

void setup_dummy_pipe_on_stdin(int pfd[2]) {
    if (pipe(pfd) == -1)
        perror("pipe failed");
    if (dup2(pfd[0], STDIN_FILENO) == -1)
        perror("dup2 failed");
}

void setup_lldb(AMDeviceRef device, CFURLRef url) {
    CFStringRef device_full_name = get_device_full_name(device),
    device_interface_name = get_device_interface_name(device);

    connect_and_start_session(device);
    CFBooleanRef is_password_protected = AMDeviceCopyValue(device, 0, CFSTR("PasswordProtected"));
    NSLogJSON(@{@"Event": @"PasswordProtectedStatus",
                @"Status": @(CFBooleanGetValue(is_password_protected)),
    });
    CFRelease(is_password_protected);

    NSLogOut(@"------ Debug phase ------");

    NSLogOut(@"Starting debug of %@ connected through %@...", device_full_name, device_interface_name);

    mount_developer_image(device);      // put debugserver on the device

    start_debug_server_multiplexer(device);  // start debugserver proxy listener
    if (debugserver_only) {
        NSLogOut(@"[100%%] Listening for lldb connections");
    }
    else {
        int connection_id = 0;
        CFNumberRef cf_connection_id = CFAutorelease(CFNumberCreate(NULL, kCFNumberIntType, &connection_id));

        create_remote_debug_server_socket(cf_connection_id, device);   // start debugserver
        write_lldb_prep_cmds(device, url);   // dump the necessary lldb commands into a file
        NSLogOut(@"[100%%] Connecting to remote debug server");
    }
    NSLogOut(@"-------------------------");

    if (url != NULL)
        CFRelease(url);

    setpgid(getpid(), 0);
    signal(SIGHUP, killed);
    signal(SIGINT, killed);
    signal(SIGTERM, killed);
    // Need this before fork to avoid race conditions. For child process we remove this right after fork.
    signal(SIGLLDB, lldb_finished_handler);

    parent = getpid();
}

void launch_debugger(AMDeviceRef device, CFURLRef url) {
    setup_lldb(device, url);
    int pid = fork();
    if (pid == 0) {
        signal(SIGHUP, SIG_DFL);
        signal(SIGLLDB, SIG_DFL);
        child = getpid();
        pid_t oldfgpid = 0;
        int pfd[2] = {-1, -1};
        if (isatty(STDIN_FILENO))
            // If we are running on a terminal, then we need to bring process to foreground for input
            // to work correctly on lldb's end.
            oldfgpid = bring_process_to_foreground();
        else
            // If lldb is running in a non terminal environment, then it freaks out spamming "^D" and
            // "quit". It seems this is caused by read() on stdin returning EOF in lldb. To hack around
            // this we setup a dummy pipe on stdin, so read() would block expecting "user's" input.
            setup_dummy_pipe_on_stdin(pfd);

        NSString* lldb_shell;
        NSString* prep_cmds = [NSString stringWithFormat:PREP_CMDS_PATH, tmpUUID];
        lldb_shell = [NSString stringWithFormat:LLDB_SHELL, prep_cmds];

        if(device_id != NULL) {
            lldb_shell = [lldb_shell stringByAppendingString: [[NSString stringWithUTF8String:device_id] stringByReplacingOccurrencesOfString:@"-" withString:@"_"]];
        }

        int status = system([lldb_shell UTF8String]); // launch lldb
        if (status == -1)
            perror("failed launching lldb");

        close(pfd[0]);
        close(pfd[1]);

        // Notify parent we're exiting
        kill(parent, SIGLLDB);

        if (oldfgpid) {
            tcsetpgrp(STDIN_FILENO, oldfgpid);
        }
        // Pass lldb exit code
        _exit(WEXITSTATUS(status));
    } else if (pid > 0) {
        child = pid;
    } else {
        on_sys_error(@"Fork failed");
    }
}

void launch_debugger_and_exit(AMDeviceRef device, CFURLRef url) {
    setup_lldb(device,url);
    int pfd[2] = {-1, -1};
    if (pipe(pfd) == -1)
        perror("Pipe failed");
    int pid = fork();
    if (pid == 0) {
        signal(SIGHUP, SIG_DFL);
        signal(SIGLLDB, SIG_DFL);
        child = getpid();

        if (dup2(pfd[0],STDIN_FILENO) == -1)
            perror("dup2 failed");


        NSString* prep_cmds = [NSString stringWithFormat:PREP_CMDS_PATH, tmpUUID];
        NSString* lldb_shell = [NSString stringWithFormat:LLDB_SHELL, prep_cmds];
        if(device_id != NULL) {
            lldb_shell = [lldb_shell stringByAppendingString:[[NSString stringWithUTF8String:device_id] stringByReplacingOccurrencesOfString:@"-" withString:@"_"]];
        }

        int status = system([lldb_shell UTF8String]); // launch lldb
        if (status == -1)
            perror("failed launching lldb");

        close(pfd[0]);

        // Notify parent we're exiting
        kill(parent, SIGLLDB);
        // Pass lldb exit code
        _exit(WEXITSTATUS(status));
    } else if (pid > 0) {
        child = pid;
        NSLogVerbose(@"Waiting for child [Child: %d][Parent: %d]\n", child, parent);
    } else {
        on_sys_error(@"Fork failed");
    }
}

void launch_debugserver_only(AMDeviceRef device, CFURLRef url)
{
    if (url != NULL)
      CFRetain(url);
    setup_lldb(device,url);

    CFStringRef device_app_path = NULL;
    if (url != NULL) {
      CFStringRef bundle_identifier = copy_disk_app_identifier(url);
      CFURLRef device_app_url = copy_device_app_url(device, bundle_identifier);
      CFRelease(bundle_identifier);
      device_app_path = CFURLCopyFileSystemPath(device_app_url, kCFURLPOSIXPathStyle);
      CFRelease(device_app_url);
      CFRelease(url);
    }

    NSLogOut(@"debugserver port: %d", port);
    if (device_app_path == NULL) {
        NSLogJSON(@{@"Event": @"DebugServerLaunched",
                    @"Port": @(port),
                    });
    } else {
        NSLogOut(@"App path: %@", device_app_path);
        NSLogJSON(@{@"Event": @"DebugServerLaunched",
                    @"Port": @(port),
                    @"Path": (__bridge NSString *)device_app_path
                    });
        CFRelease(device_app_path);
    }
}

CFStringRef copy_bundle_id(CFURLRef app_url)
{
    if (app_url == NULL)
        return NULL;

    CFURLRef url = CFURLCreateCopyAppendingPathComponent(NULL, app_url, CFSTR("Info.plist"), false);

    if (url == NULL)
        return NULL;

    CFReadStreamRef stream = CFReadStreamCreateWithFile(NULL, url);
    CFRelease(url);

    if (stream == NULL)
        return NULL;

    CFPropertyListRef plist = NULL;
    if (CFReadStreamOpen(stream) == TRUE) {
        plist = CFPropertyListCreateWithStream(NULL, stream, 0,
                                               kCFPropertyListImmutable, NULL, NULL);
    }
    CFReadStreamClose(stream);
    CFRelease(stream);

    if (plist == NULL)
        return NULL;

    const void *value = CFDictionaryGetValue(plist, CFSTR("CFBundleIdentifier"));
    CFStringRef bundle_id = NULL;
    if (value != NULL)
        bundle_id = CFRetain(value);

    CFRelease(plist);
    return bundle_id;
}

typedef enum { READ_DIR_FILE, READ_DIR_FIFO, READ_DIR_BEFORE_DIR, READ_DIR_AFTER_DIR } read_dir_cb_reason;

void read_dir(AFCConnectionRef afc_conn_p, const char* dir,
              void(*callback)(AFCConnectionRef conn, const char *dir, read_dir_cb_reason reason))
{
    char *dir_ent;

    afc_dictionary* afc_dict_p;
    char *key, *val;
    int not_dir = 0;
    bool is_fifo = 0;

    unsigned int code = AFCFileInfoOpen(afc_conn_p, dir, &afc_dict_p);
    if (code != 0) {
        // there was a problem reading or opening the file to get info on it, abort
        return;
    }
    
    long long mtime = -1;
    long long birthtime = -1;
    long size = -1;
    long blocks = -1;
    long nlink = -1;
    NSString * ifmt = nil;
    while((AFCKeyValueRead(afc_dict_p,&key,&val) == 0) && key && val) {
        if (strcmp(key,"st_ifmt")==0) {
            not_dir = strcmp(val,"S_IFDIR");
            is_fifo = !strcmp(val, "S_IFIFO");
            if (_json_output) {
                ifmt = [NSString stringWithUTF8String:val];
            } else {
                break;
            }
        } else if (strcmp(key, "st_size") == 0) {
            size = atol(val);
        } else if (strcmp(key, "st_mtime") == 0) {
            mtime = atoll(val);
        } else if (strcmp(key, "st_birthtime") == 0) {
            birthtime = atoll(val);
        } else if (strcmp(key, "st_nlink") == 0) {
            nlink = atol(val);
        } else if (strcmp(key, "st_blocks") == 0) {
            blocks = atol(val);
        }
    }
    AFCKeyValueClose(afc_dict_p);
    
    if (_json_output) {
        if (_file_meta_info == nil) {
            _file_meta_info = [[NSMutableArray alloc] init];
        }
        [_file_meta_info addObject: @{@"full_path": [NSString stringWithUTF8String:dir],
                                      @"st_ifmt": ifmt,
                                      @"st_nlink": @(nlink),
                                      @"st_size": @(size),
                                      @"st_blocks": @(blocks),
                                      @"st_mtime": @(mtime),
                                      @"st_birthtime": @(birthtime)}];
    }
    
    if (not_dir) {
        if (callback) (*callback)(afc_conn_p, dir, is_fifo ? READ_DIR_FIFO : READ_DIR_FILE);
        return;
    }

    afc_directory* afc_dir_p;
    afc_error_t err = AFCDirectoryOpen(afc_conn_p, dir, &afc_dir_p);

    if (err != 0) {
        // Couldn't open dir - was probably a file
        return;
    }
    
    // Call the callback on the directory before processing its
    // contents. This is used by copy file callback, which needs to
    // create the directory on the host before attempting to copy
    // files into it.
    if (callback) (*callback)(afc_conn_p, dir, READ_DIR_BEFORE_DIR);

    while(true) {
        err = AFCDirectoryRead(afc_conn_p, afc_dir_p, &dir_ent);

        if (err != 0 || !dir_ent)
            break;

        if (strcmp(dir_ent, ".") == 0 || strcmp(dir_ent, "..") == 0)
            continue;

        char* dir_joined = malloc(strlen(dir) + strlen(dir_ent) + 2);
        strcpy(dir_joined, dir);
        if (dir_joined[strlen(dir)-1] != '/')
            strcat(dir_joined, "/");
        strcat(dir_joined, dir_ent);
        if (!(non_recursively && strcmp(list_root, dir) != 0)) {
            read_dir(afc_conn_p, dir_joined, callback);
        }
        free(dir_joined);
    }

    AFCDirectoryClose(afc_conn_p, afc_dir_p);
    
    // Call the callback on the directory after processing its
    // contents. This is used by the rmtree callback because it needs
    // to delete the directory's contents before the directory itself
    if (callback) (*callback)(afc_conn_p, dir, READ_DIR_AFTER_DIR);
}

AFCConnectionRef start_afc_service(AMDeviceRef device) {
    AMDeviceConnect(device);
    assert(AMDeviceIsPaired(device));
    check_error(AMDeviceValidatePairing(device));
    check_error(AMDeviceStartSession(device));

    AFCConnectionRef conn = NULL;
    ServiceConnRef serviceConn = NULL;

    if (AMDeviceStartService(device, AMSVC_AFC, &serviceConn, 0) != MDERR_OK) {
        on_error(@"Unable to start file service!");
    }
    if (AFCConnectionOpen(serviceConn, 0, &conn) != MDERR_OK) {
        on_error(@"Unable to open connection!");
    }

    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
    return conn;
}

// Used to send files to app-specific sandbox (Documents dir)
AFCConnectionRef start_house_arrest_service(AMDeviceRef device) {
    connect_and_start_session(device);

    AFCConnectionRef conn = NULL;

    if (bundle_id == NULL) {
        on_error(@"Bundle id is not specified");
    }

    CFStringRef cf_bundle_id = CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8);
    CFStringRef keys[1];
    keys[0] = CFSTR("Command");
    CFStringRef values[1];
    values[0] = CFSTR("VendDocuments");
    CFDictionaryRef command = CFDictionaryCreate(kCFAllocatorDefault,
                                                 (void*)keys,
                                                 (void*)values,
                                                 1,
                                                 &kCFTypeDictionaryKeyCallBacks,
                                                 &kCFTypeDictionaryValueCallBacks);
    if (AMDeviceCreateHouseArrestService(device, cf_bundle_id, 0, &conn) != 0 &&
        AMDeviceCreateHouseArrestService(device, cf_bundle_id, command, &conn) != 0) {
        on_error(@"Unable to find bundle with id: %@", [NSString stringWithUTF8String:bundle_id]);
    }

    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
    CFRelease(cf_bundle_id);

    return conn;
}

// Uses realpath() to resolve any symlinks in a path. Returns the resolved
// path or the original path if an error occurs. This allocates memory for the
// resolved path and the caller is responsible for freeing it.
char *resolve_path(char *path)
{
  char buffer[PATH_MAX];
  // Use the original path if realpath() fails, otherwise use resolved value.
  char *resolved_path = realpath(path, buffer) == NULL ? path : buffer;
  char *new_path = malloc(strlen(resolved_path) + 1);
  strcpy(new_path, resolved_path);
  return new_path;
}

char const* get_filename_from_path(char const* path)
{
    char const*ptr = path + strlen(path);
    while (ptr > path)
    {
        if (*ptr == '/')
            break;
        --ptr;
    }
    if (ptr+1 >= path+strlen(path))
        return NULL;
    if (ptr == path)
        return ptr;
    return ptr+1;
}

void* read_file_to_memory(char const * path, size_t* file_size)
{
    struct stat buf;
    int err = stat(path, &buf);
    if (err < 0)
    {
        return NULL;
    }

    *file_size = buf.st_size;
    FILE* fd = fopen(path, "r");
    char* content = malloc(*file_size);
    if (*file_size != 0 && fread(content, *file_size, 1, fd) != 1)
    {
        fclose(fd);
        return NULL;
    }
    fclose(fd);
    return content;
}

void list_files_callback(AFCConnectionRef conn, const char *name, read_dir_cb_reason reason)
{
    if (reason == READ_DIR_FILE || reason == READ_DIR_FIFO) {
        NSLogOut(@"%@", [NSString stringWithUTF8String:name]);
    } else if (reason == READ_DIR_BEFORE_DIR) {
        NSLogOut(@"%@/", [NSString stringWithUTF8String:name]);
    }
}

void list_files(AMDeviceRef device)
{
    AFCConnectionRef afc_conn_p;
    if (file_system) {
        afc_conn_p = start_afc_service(device);
    } else {
        afc_conn_p = start_house_arrest_service(device);
    }
    assert(afc_conn_p);
    if (_json_output) {
        read_dir(afc_conn_p, list_root?list_root:"/", NULL);
        NSLogJSON(@{@"Event": @"FileListed",
                    @"Files": _file_meta_info});
    } else {
        read_dir(afc_conn_p, list_root?list_root:"/", list_files_callback);
    }
    
    check_error(AFCConnectionClose(afc_conn_p));
}

int app_exists(AMDeviceRef device)
{
    if (bundle_id == NULL) {
        NSLogOut(@"Bundle id is not specified.");
        return 1;
    }
    AMDeviceConnect(device);
    assert(AMDeviceIsPaired(device));
    check_error(AMDeviceValidatePairing(device));
    check_error(AMDeviceStartSession(device));

    CFStringRef cf_bundle_id = CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8);

    NSArray *a = [NSArray arrayWithObjects:@"CFBundleIdentifier", nil];
    NSDictionary *optionsDict = [NSDictionary dictionaryWithObject:a forKey:@"ReturnAttributes"];
    CFDictionaryRef options = (CFDictionaryRef)optionsDict;
    CFDictionaryRef result = nil;
    check_error(AMDeviceLookupApplications(device, options, &result));

    bool appExists = CFDictionaryContainsKey(result, cf_bundle_id);
    NSLogOut(@"%@", appExists ? @"true" : @"false");
    CFRelease(cf_bundle_id);

    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
    if (appExists)
        return 0;
    return -1;
}

void get_battery_level(AMDeviceRef device)
{
    
    AMDeviceConnect(device);
    assert(AMDeviceIsPaired(device));
    check_error(AMDeviceValidatePairing(device));
    check_error(AMDeviceStartSession(device));

    CFStringRef result = AMDeviceCopyValue(device, (void*)@"com.apple.mobile.battery", (__bridge CFStringRef)@"BatteryCurrentCapacity");
    NSLogOut(@"BatteryCurrentCapacity:%@",result);
    CFRelease(result);
    
    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
}

void replace_dict_date_with_absolute_time(CFMutableDictionaryRef dict, CFStringRef key) {
    CFDateRef date = CFDictionaryGetValue(dict, key);
    CFAbsoluteTime absolute_date = CFDateGetAbsoluteTime(date);
    CFNumberRef absolute_date_ref = CFNumberCreate(NULL, kCFNumberDoubleType, &absolute_date);
    CFDictionaryReplaceValue(dict, key, absolute_date_ref);
    CFRelease(absolute_date_ref);
}

void list_provisioning_profiles(AMDeviceRef device) {
    connect_and_start_session(device);
    CFArrayRef device_provisioning_profiles = AMDeviceCopyProvisioningProfiles(device);

    CFIndex provisioning_profiles_count = CFArrayGetCount(device_provisioning_profiles);
    CFMutableArrayRef serializable_provisioning_profiles =
        CFArrayCreateMutable(NULL, provisioning_profiles_count, &kCFTypeArrayCallBacks);

    for (CFIndex i = 0; i < provisioning_profiles_count; i++) {
        void *device_provisioning_profile =
            (void *)CFArrayGetValueAtIndex(device_provisioning_profiles, i);
        CFMutableDictionaryRef serializable_provisioning_profile;

        if (verbose) {
            // Verbose output; We selectively omit keys from profile.
            CFDictionaryRef immutable_profile_dict =
                MISProfileCopyPayload(device_provisioning_profile);
            serializable_provisioning_profile =
                CFDictionaryCreateMutableCopy(kCFAllocatorDefault, 0, immutable_profile_dict);
            CFRelease(immutable_profile_dict);

            // Remove binary values from the output since they aren't readable and add a whole lot
            // of noise to the output.
            CFDictionaryRemoveValue(serializable_provisioning_profile,
                                    CFSTR("DER-Encoded-Profile"));
            CFDictionaryRemoveValue(serializable_provisioning_profile,
                                    CFSTR("DeveloperCertificates"));
        } else {
            // Normal output; We selectively include keys from profile.
            CFStringRef keys[] = {CFSTR("Name"), CFSTR("UUID"), CFSTR("ExpirationDate")};
            CFIndex size = sizeof(keys) / sizeof(CFStringRef);
            serializable_provisioning_profile =
                CFDictionaryCreateMutable(kCFAllocatorDefault, size, &kCFTypeDictionaryKeyCallBacks,
                                          &kCFTypeDictionaryValueCallBacks);
            for (CFIndex i = 0; i < size; i++) {
                CFStringRef key = keys[i];
                CFStringRef value = MISProfileGetValue(device_provisioning_profile, key);
                CFDictionaryAddValue(serializable_provisioning_profile, key, value);
            }
        }

        if (_json_output) {
            // JSON output can't have CFDate objects so convert dates into CFAbsoluteTime's.
            replace_dict_date_with_absolute_time(serializable_provisioning_profile,
                                                 CFSTR("ExpirationDate"));
            replace_dict_date_with_absolute_time(serializable_provisioning_profile,
                                                 CFSTR("CreationDate"));
        }

        CFArrayAppendValue(serializable_provisioning_profiles, serializable_provisioning_profile);
        CFRelease(serializable_provisioning_profile);
    }
    CFRelease(device_provisioning_profiles);

    if (_json_output) {
        NSLogJSON(@{
            @"Event" : @"ListProvisioningProfiles",
            @"Profiles" : (NSArray *)serializable_provisioning_profiles
        });
    } else {
        NSLogOut(@"%@", serializable_provisioning_profiles);
    }
    CFRelease(serializable_provisioning_profiles);
}

void install_provisioning_profile(AMDeviceRef device) {
    if (!profile_path) {
        on_error(@"no path to provisioning profile specified");
    }

    size_t file_size = 0;
    void *file_content = read_file_to_memory(profile_path, &file_size);
    CFDataRef profile_data = CFDataCreate(NULL, file_content, file_size);
    void *profile = MISProfileCreateWithData(0, profile_data);
    connect_and_start_session(device);
    check_error(AMDeviceInstallProvisioningProfile(device, profile));

    free(file_content);
    CFRelease(profile_data);
    CFRelease(profile);
}

void uninstall_provisioning_profile(AMDeviceRef device) {
    if (!profile_uuid) {
        on_error(@"no profile UUID specified via --profile-uuid");
    }

    CFStringRef uuid = CFStringCreateWithCString(NULL, profile_uuid, kCFStringEncodingUTF8);
    connect_and_start_session(device);
    check_error(AMDeviceRemoveProvisioningProfile(device, uuid));
    CFRelease(uuid);
}

void download_provisioning_profile(AMDeviceRef device) {
    if (!profile_uuid) {
        on_error(@"no profile UUID specified via --profile-uuid");
    } else if (!profile_path) {
        on_error(@"no download path specified");
    }

    connect_and_start_session(device);
    CFArrayRef device_provisioning_profiles = AMDeviceCopyProvisioningProfiles(device);
    CFIndex provisioning_profiles_count = CFArrayGetCount(device_provisioning_profiles);
    CFStringRef uuid = CFStringCreateWithCString(NULL, profile_uuid, kCFStringEncodingUTF8);
    bool found_matching_uuid = false;

    for (CFIndex i = 0; i < provisioning_profiles_count; i++) {
        void *profile = (void *)CFArrayGetValueAtIndex(device_provisioning_profiles, i);
        CFStringRef other_uuid = MISProfileGetValue(profile, CFSTR("UUID"));
        found_matching_uuid = CFStringCompare(uuid, other_uuid, 0) == kCFCompareEqualTo;

        if (found_matching_uuid) {
            NSLogVerbose(@"Writing %@ to %s", MISProfileGetValue(profile, CFSTR("Name")),
                         profile_path);
            CFStringRef dst_path =
                CFStringCreateWithCString(NULL, profile_path, kCFStringEncodingUTF8);
            check_error(MISProfileWriteToFile(profile, dst_path));
            CFRelease(dst_path);
            break;
        }
    }

    CFRelease(uuid);
    CFRelease(device_provisioning_profiles);
    if (!found_matching_uuid) {
        on_error(@"Did not find provisioning profile with UUID %x on device", profile_uuid);
    }
}

void list_bundle_id(AMDeviceRef device)
{
    connect_and_start_session(device);
    NSMutableArray *a = [NSMutableArray arrayWithObjects:
                         @"CFBundleIdentifier",
                         @"CFBundleName",
                         @"CFBundleDisplayName",
                         @"CFBundleVersion",
                         @"CFBundleShortVersionString", nil];
    if (keys) {
        for (NSString * key in keys) {
            [a addObjectsFromArray:[key componentsSeparatedByCharactersInSet:
                                    [NSCharacterSet characterSetWithCharactersInString:@",&"]]];
        }
    }
    NSDictionary *optionsDict = [NSDictionary dictionaryWithObject:a forKey:@"ReturnAttributes"];
    CFDictionaryRef options = (CFDictionaryRef)optionsDict;
    CFDictionaryRef result = nil;
    check_error(AMDeviceLookupApplications(device, options, &result));

    if (bundle_id != NULL) {
        CFStringRef cf_bundle_id = CFAutorelease(CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8));
        CFDictionaryRef app_dict = CFRetain(CFDictionaryGetValue(result, cf_bundle_id));

        CFRelease(result);
        result = CFAutorelease(CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));

        if (app_dict != NULL) {
            CFDictionaryAddValue((CFMutableDictionaryRef)result, cf_bundle_id, app_dict);
            CFRelease(app_dict);
        }
    }

    if (_json_output) {
        NSLogJSON(@{@"Event": @"ListBundleId",
                    @"Apps": (NSDictionary *)result});
    } else {
        CFIndex count;
        count = CFDictionaryGetCount(result);
        const void *keys[count];
        CFDictionaryGetKeysAndValues(result, keys, NULL);
        for(int i = 0; i < count; ++i) {
            NSLogOut(@"%@", (CFStringRef)keys[i]);
        }
    }

    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
}

void copy_file_callback(AFCConnectionRef afc_conn_p, const char *name, read_dir_cb_reason reason)
{
    const char *local_name=name;

    if (*local_name=='/') local_name++;

    if (*local_name=='\0') return;

    if (reason == READ_DIR_FILE || reason == READ_DIR_FIFO) {
        NSLogOut(@"%@", [NSString stringWithUTF8String:name]);
        afc_file_ref fref;
        int err = AFCFileRefOpen(afc_conn_p,name,1,&fref);

        if (err) {
            fprintf(stderr,"AFCFileRefOpen(\"%s\") failed: %d\n",name,err);
            return;
        }

        FILE *fp = fopen(local_name,"w");

        if (fp==NULL) {
            fprintf(stderr,"fopen(\"%s\",\"w\") failer: %s\n",local_name,strerror(errno));
            AFCFileRefClose(afc_conn_p,fref);
            return;
        }

        char buf[4096];
        size_t sz=sizeof(buf);

        while (AFCFileRefRead(afc_conn_p,fref,buf,&sz)==0 && sz) {
            fwrite(buf,sz,1,fp);
            sz = sizeof(buf);
        }

        AFCFileRefClose(afc_conn_p,fref);
        fclose(fp);

    } else if (reason == READ_DIR_BEFORE_DIR) {
        NSLogOut(@"%@/", [NSString stringWithUTF8String:name]);
        if (mkdir(local_name,0777) && errno!=EEXIST) {
            fprintf(stderr,"mkdir(\"%s\") failed: %s\n",local_name,strerror(errno));
        }
    }
}


void download_tree(AMDeviceRef device)
{
    AFCConnectionRef afc_conn_p;
    if (file_system) {
        afc_conn_p = start_afc_service(device);
    } else {
        afc_conn_p = start_house_arrest_service(device);
    }
    
    assert(afc_conn_p);
    char *dirname = NULL;

    list_root = list_root? list_root : "/";
    target_filename = target_filename? target_filename : ".";

    NSString* targetPath = [NSString pathWithComponents:@[ @(target_filename), @(list_root)] ];
    mkdirp([targetPath stringByDeletingLastPathComponent]);

    do {
        if (target_filename) {
            dirname = strdup(target_filename);
            mkdirp(@(dirname));
            if (mkdir(dirname,0777) && errno!=EEXIST) {
                fprintf(stderr,"mkdir(\"%s\") failed: %s\n",dirname,strerror(errno));
                break;
            }
            if (chdir(dirname)) {
                fprintf(stderr,"chdir(\"%s\") failed: %s\n",dirname,strerror(errno));
                break;
            }
        }
        read_dir(afc_conn_p, list_root, copy_file_callback);
    } while(0);

    if (dirname) free(dirname);
    if (afc_conn_p) AFCConnectionClose(afc_conn_p);
}

void upload_dir(AMDeviceRef device, AFCConnectionRef afc_conn_p, NSString* source, NSString* destination);
void upload_single_file(AMDeviceRef device, AFCConnectionRef afc_conn_p, NSString* sourcePath, NSString* destinationPath);

void upload_file(AMDeviceRef device)
{
    AFCConnectionRef afc_conn_p;
    if (file_system) {
        afc_conn_p = start_afc_service(device);
    } else {
        afc_conn_p = start_house_arrest_service(device);
    }
    assert(afc_conn_p);

    if (!target_filename)
    {
        target_filename = get_filename_from_path(upload_pathname);
    }

    NSString* sourcePath = [NSString stringWithUTF8String: upload_pathname];
    NSString* destinationPath = [NSString stringWithUTF8String: target_filename];

    BOOL isDir;
    bool exists = [[NSFileManager defaultManager] fileExistsAtPath: sourcePath isDirectory: &isDir];
    if (!exists)
    {
        on_error(@"Could not find file: %s", upload_pathname);
    }
    else if (isDir)
    {
        upload_dir(device, afc_conn_p, sourcePath, destinationPath);
    }
    else
    {
        upload_single_file(device, afc_conn_p, sourcePath, destinationPath);
    }
    check_error(AFCConnectionClose(afc_conn_p));
}

void upload_single_file(AMDeviceRef device, AFCConnectionRef afc_conn_p, NSString* sourcePath, NSString* destinationPath) {

    afc_file_ref file_ref;

    size_t file_size;
    void* file_content = read_file_to_memory([sourcePath fileSystemRepresentation], &file_size);

    if (!file_content)
    {
        on_error(@"Could not open file: %@", sourcePath);
    }

    // Make sure the directory was created
    {
        NSString *dirpath = [destinationPath stringByDeletingLastPathComponent];
        check_error(AFCDirectoryCreate(afc_conn_p, [dirpath fileSystemRepresentation]));
    }

    NSLogVerbose(@"%@", destinationPath);
    NSLogJSON(@{@"Event": @"UploadFile",
                @"Destination": destinationPath
                });

    int ret = AFCFileRefOpen(afc_conn_p, [destinationPath fileSystemRepresentation], 3, &file_ref);
    if (ret == 0x000a) {
        on_error(@"Cannot write to %@. Permission error.", destinationPath);
    }
    if (ret == 0x0009) {
        on_error(@"Target %@ is a directory.", destinationPath);
    }
    assert(ret == 0);
    check_error(AFCFileRefWrite(afc_conn_p, file_ref, file_content, file_size));
    check_error(AFCFileRefClose(afc_conn_p, file_ref));

    free(file_content);
}

void upload_dir(AMDeviceRef device, AFCConnectionRef afc_conn_p, NSString* source, NSString* destination)
{
    check_error(AFCDirectoryCreate(afc_conn_p, [destination fileSystemRepresentation]));
    for (NSString* item in [[NSFileManager defaultManager] contentsOfDirectoryAtPath: source error: nil])
    {
        NSString* sourcePath = [source stringByAppendingPathComponent: item];
        NSString* destinationPath = [destination stringByAppendingPathComponent: item];
        BOOL isDir;
        [[NSFileManager defaultManager] fileExistsAtPath: sourcePath isDirectory: &isDir];
        if (isDir)
        {
            NSString *dirDestinationPath = [destinationPath stringByAppendingString:@"/"];
            NSLogVerbose(@"%@", dirDestinationPath);
            NSLogJSON(@{@"Event": @"UploadDir",
                        @"Destination": dirDestinationPath
                        });
            upload_dir(device, afc_conn_p, sourcePath, destinationPath);
        }
        else
        {
            upload_single_file(device, afc_conn_p, sourcePath, destinationPath);
        }
    }
}

void make_directory(AMDeviceRef device) {
    AFCConnectionRef afc_conn_p;
    if (file_system) {
        afc_conn_p = start_afc_service(device);
    } else {
        afc_conn_p = start_house_arrest_service(device);
    }
    assert(afc_conn_p);
    check_error(AFCDirectoryCreate(afc_conn_p, target_filename));
    check_error(AFCConnectionClose(afc_conn_p));
}

void remove_path(AMDeviceRef device) {
    AFCConnectionRef afc_conn_p;
    if (file_system) {
        afc_conn_p = start_afc_service(device);
    } else {
        afc_conn_p = start_house_arrest_service(device);
    }
    assert(afc_conn_p);
    check_error(AFCRemovePath(afc_conn_p, target_filename));
    check_error(AFCConnectionClose(afc_conn_p));
}

// Handles the READ_DIR_AFTER_DIR callback so that we delete the contents of the
// directory before the directory itself
void rmtree_callback(AFCConnectionRef conn, const char *name, read_dir_cb_reason reason)
{
    if (reason == READ_DIR_FILE || reason == READ_DIR_AFTER_DIR) {
        NSLogVerbose(@"Deleting %s", name);
        log_error(AFCRemovePath(conn, name));
    } else if (reason == READ_DIR_FIFO) {
        NSLogVerbose(@"Skipping %s", name);
    }
}

void rmtree(AMDeviceRef device) {
    AFCConnectionRef afc_conn_p = start_house_arrest_service(device);
    assert(afc_conn_p);
    read_dir(afc_conn_p, target_filename, rmtree_callback);
    check_error(AFCConnectionClose(afc_conn_p));
}

void uninstall_app(AMDeviceRef device) {
    CFRetain(device); // don't know if this is necessary?

    NSLogOut(@"------ Uninstall phase ------");

    //Do we already have the bundle_id passed in via the command line? if so, use it.
    CFStringRef cf_uninstall_bundle_id = NULL;
    if (bundle_id != NULL)
    {
        cf_uninstall_bundle_id = CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8);
    } else {
        on_error(@"Error: you need to pass in the bundle id, (i.e. --bundle_id com.my.app)");
    }

    if (cf_uninstall_bundle_id == NULL) {
        on_error(@"Error: Unable to get bundle id from user command or package %@.\nUninstall failed.", [NSString stringWithUTF8String:app_path]);
    } else {
        connect_and_start_session(device);

        int code = AMDeviceSecureUninstallApplication(0, device, cf_uninstall_bundle_id, 0, NULL, 0);
        if (code == 0) {
            NSLogOut(@"[ OK ] Uninstalled package with bundle id %@", cf_uninstall_bundle_id);
        } else {
            on_error(@"[ ERROR ] Could not uninstall package with bundle id %@", cf_uninstall_bundle_id);
        }
        CFRelease(cf_uninstall_bundle_id);
        check_error(AMDeviceStopSession(device));
        check_error(AMDeviceDisconnect(device));
    }
}

#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
void check_developer_mode(AMDeviceRef device) {
  unsigned int error_code = 0;
  bool is_enabled = AMDeviceCopyDeveloperModeStatus(device, &error_code);

  if (error_code) {
    const char *mobdev_error = get_error_message(error_code);
    NSString *error_description = mobdev_error ? [NSString stringWithUTF8String:mobdev_error] : @"unknown.";
    if (_json_output) {
      NSLogJSON(@{
        @"Event": @"DeveloperMode",
        @"IsEnabled": @(is_enabled),
        @"Code": @(error_code),
        @"Status": error_description,
      });
    } else {
      NSLogOut(@"Encountered error checking developer mode status: %@", error_description);
    }
  } else {
    if (_json_output) {
      NSLogJSON(@{@"Event": @"DeveloperMode", @"IsEnabled": @(is_enabled)});
    } else {
      NSLogOut(@"Developer mode is%s enabled.", is_enabled ? "" : " not");
    }
  }
}
#endif

ServiceConnRef symbolsServiceConnection = NULL;

void start_symbols_service_with_command(AMDeviceRef device, uint32_t command) {
    connect_and_start_session(device);
    check_error(AMDeviceSecureStartService(device, symbols_service_name,
                                           NULL, &symbolsServiceConnection));

    uint32_t bytes_sent = AMDServiceConnectionSend(symbolsServiceConnection, &command,
                                                    sizeof_uint32_t);
    if (bytes_sent != sizeof_uint32_t) {
        on_error(@"Sent %d bytes but was expecting %d.", bytes_sent, sizeof_uint32_t);
    }

    uint32_t response;
    uint32_t bytes_read = AMDServiceConnectionReceive(symbolsServiceConnection,
                                                        &response, sizeof_uint32_t);
    if (bytes_read != sizeof_uint32_t) {
        on_error(@"Read %d bytes but was expecting %d.", bytes_read, sizeof_uint32_t);
    } else if (response != command) {
        on_error(@"Failed to get confirmation response for: %s", command);
    }
}

CFArrayRef get_dyld_file_paths(AMDeviceRef device) {
    start_symbols_service_with_command(device, symbols_file_paths_command);

    CFPropertyListFormat format;
    CFDictionaryRef dict = NULL;
    uint64_t bytes_read =
        AMDServiceConnectionReceiveMessage(symbolsServiceConnection, &dict, &format);
    if (bytes_read == -1) {
        on_error(@"Received %d bytes after succesfully starting command %d.", bytes_read,
                 symbols_file_paths_command);
    }
    AMDeviceStopSession(device);
    AMDeviceDisconnect(device);

    CFStringRef files_key = CFSTR("files");
    if (!CFDictionaryContainsKey(dict, files_key)) {
        on_error(@"Incoming messasge did not contain key '%@', %@", files_key, dict);
    }
    return CFDictionaryGetValue(dict, files_key);
}

void write_dyld_file(CFStringRef dest, uint64_t file_size) {
    // Prepare the destination file by mapping it into memory.
    int fd = open(CFStringGetCStringPtr(dest, kCFStringEncodingUTF8),
                  O_RDWR | O_CREAT, 0644);
    if (fd == -1) {
        on_sys_error(@"Failed to open %@.", dest);
    }
    if (lseek(fd, file_size - 1, SEEK_SET) == -1) {
        on_sys_error(@"Failed to lseek to last byte.");
    }
    if (write(fd, "", 1) == -1) {
        on_sys_error(@"Failed to write to last byte.");
    }
    void *map = mmap(NULL, file_size, PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) {
        on_sys_error(@"Failed to mmap %@.", dest);
    }
    close(fd);

    // Read the file content packet by packet until we've copied the entire file
    // to disk.
    uint64_t total_bytes_read = 0;
    uint64_t last_time =
        get_current_time_in_milliseconds() / symbols_logging_interval_ms;
    while (total_bytes_read < file_size) {
        uint64_t bytes_remaining = file_size - total_bytes_read;
        // This fails for some reason if we try to download more than
        // INT_MAX bytes at a time.
        uint64_t bytes_to_download = MIN(bytes_remaining, INT_MAX - 1);
        uint64_t bytes_read = AMDServiceConnectionReceive(
            symbolsServiceConnection, map + total_bytes_read, bytes_to_download);
        total_bytes_read += bytes_read;

        uint64_t current_time =
            get_current_time_in_milliseconds() / symbols_logging_interval_ms;
        // We can process several packets per second which would result
        // in spamming output so only log if any of the following are
        // true:
        //    - Running in verbose mode.
        //    - It's been at least a quarter second since the last log.
        //    - We finished processing the last packet.
        if (verbose || last_time != current_time || total_bytes_read == file_size) {
            last_time = current_time;
            int percent = (double)total_bytes_read / file_size * 100;
            NSLogOut(@"%llu/%llu (%d%%)", total_bytes_read, file_size, percent);
            NSLogJSON(@{@"Event": @"DyldCacheDownloadProgress",
                         @"BytesRead": @(total_bytes_read),
                         @"Percent": @(percent),
                      });
        }
    }

    munmap(map, file_size);
}

CFStringRef download_dyld_file(AMDeviceRef device, uint32_t dyld_index,
                        CFStringRef filepath) {
    start_symbols_service_with_command(device, symbols_download_file_command);

    uint32_t index = CFSwapInt32HostToBig(dyld_index);
    uint64_t bytes_sent =
        AMDServiceConnectionSend(symbolsServiceConnection, &index, sizeof_uint32_t);
    if (bytes_sent != sizeof_uint32_t) {
        on_error(@"Sent %d bytes but was expecting %d.", bytes_sent, sizeof_uint32_t);
    }

    uint64_t file_size = 0;
    uint64_t bytes_read = AMDServiceConnectionReceive(symbolsServiceConnection,
                                                    &file_size, sizeof(uint64_t));
    if (bytes_read != sizeof(uint64_t)) {
        on_error(@"Read %d bytes but was expecting %d.", bytes_read, sizeof(uint64_t));
    }
    file_size = CFSwapInt64BigToHost(file_size);

    CFStringRef download_path = CFStringCreateWithFormat(
        NULL, NULL, CFSTR("%s%@"), symbols_download_directory, filepath);
    mkdirp(
        ((__bridge NSString *)download_path).stringByDeletingLastPathComponent);
    NSLogOut(@"Downloading %@ to %@.", filepath, download_path);
    NSLogJSON(@{@"Event": @"DyldCacheDownload",
                 @"Source": (__bridge NSString *)filepath,
                 @"Destination": (__bridge NSString *)download_path,
                 @"Size": @(file_size),
              });

    write_dyld_file(download_path, file_size);

    AMDeviceStopSession(device);
    AMDeviceDisconnect(device);
    return download_path;
}

CFStringRef create_dsc_bundle_path_for_device(AMDeviceRef device) {
    CFStringRef xcode_dev_path = copy_xcode_dev_path();

    is_usb_device(device) ? AMDeviceConnect(device) : connect_and_start_session(device);
    CFStringRef device_class = AMDeviceCopyValue(device, 0, CFSTR("DeviceClass"));
    AMDeviceDisconnect(device);
    if (!device_class) {
      on_error(@"Failed to determine device class");
    }

    CFStringRef platform_name;
    if (CFStringCompare(CFSTR("AppleTV"), device_class, 0) == kCFCompareEqualTo) {
        platform_name = CFSTR("AppleTVOS");
    } else if (CFStringCompare(CFSTR("Watch"), device_class, 0) ==
               kCFCompareEqualTo) {
        platform_name = CFSTR("WatchOS");
    } else {
        platform_name = CFSTR("iPhoneOS");
    }

    return CFStringCreateWithFormat(
        NULL, NULL,
        CFSTR("%@/Platforms/%@.platform/usr/lib/dsc_extractor.bundle"),
        xcode_dev_path, platform_name);
}

typedef int (*extractor_proc)(const char *shared_cache_file_path, const char *extraction_root_path,
                              void (^progress)(unsigned current, unsigned total));

void dyld_shared_cache_extract_dylibs(CFStringRef dsc_extractor_bundle_path,
                                      CFStringRef shared_cache_file_path,
                                      const char *extraction_root_path) {
    const char *dsc_extractor_bundle_path_ptr =
        CFStringGetCStringPtr(dsc_extractor_bundle_path, kCFStringEncodingUTF8);
    void *handle = dlopen(dsc_extractor_bundle_path_ptr, RTLD_LAZY);
    if (handle == NULL) {
        on_error(@"%s could not be loaded", dsc_extractor_bundle_path);
    }

    extractor_proc proc = (extractor_proc)dlsym(
        handle, "dyld_shared_cache_extract_dylibs_progress");
    if (proc == NULL) {
        on_error(
            @"%s did not have dyld_shared_cache_extract_dylibs_progress symbol",
            dsc_extractor_bundle_path);
    }

    const char *shared_cache_file_path_ptr =
        CFStringGetCStringPtr(shared_cache_file_path, kCFStringEncodingUTF8);
  
    NSLogJSON(@{@"Event": @"DyldCacheExtract",
                 @"Source": (__bridge NSString *)shared_cache_file_path,
                 @"Destination": @(extraction_root_path),
              });

    __block uint64_t last_time =
        get_current_time_in_milliseconds() / symbols_logging_interval_ms;
    __block unsigned files_extracted = 0;
    __block unsigned files_total = 0;
    int result =
        (*proc)(shared_cache_file_path_ptr, extraction_root_path,
                ^(unsigned c, unsigned total) {
              uint64_t current_time =
                  get_current_time_in_milliseconds() / symbols_logging_interval_ms;
              if (!verbose && last_time == current_time) return;

              last_time = current_time;
              files_extracted = c;
              files_total = total;
          
              int percent = (double)c / total * 100;
              NSLogOut(@"%d/%d (%d%%)", c, total, percent);
              NSLogJSON(@{@"Event": @"DyldCacheExtractProgress",
                           @"Extracted": @(c),
                           @"Total": @(total),
                           @"Percent": @(percent),
                        });
        });
    if (result == 0) {
        NSLogOut(@"Finished extracting %s.", shared_cache_file_path_ptr);
        files_extracted = files_total;
    } else {
        NSLogOut(@"Failed to extract %s, exit code %d.", shared_cache_file_path_ptr, result);
    }
    int percent = (double)files_extracted / files_total * 100;
    NSLogJSON(@{@"Event": @"DyldCacheExtractProgress",
                @"Code": @(result),
                @"Extracted": @(files_extracted),
                @"Total": @(files_total),
                @"Percent": @(percent),
              });
}

void download_device_symbols(AMDeviceRef device) {
    symbolsServiceConnection = NULL;
    CFArrayRef files = get_dyld_file_paths(device);
    CFIndex files_count = CFArrayGetCount(files);
    NSLogOut(@"Downloading symbols files: %@", files);
    NSLogJSON(@{@"Event": @"SymbolsDownload",
                 @"Files": (__bridge NSArray *)files,
              });
    CFStringRef dsc_extractor_bundle = create_dsc_bundle_path_for_device(device);
    CFMutableArrayRef downloaded_files = CFArrayCreateMutable(NULL, 0, &kCFTypeArrayCallBacks);

    // download files
    for (uint32_t i = 0; i < files_count; ++i) {
        CFStringRef filepath = (CFStringRef)CFArrayGetValueAtIndex(files, i);
        CFStringRef download_path = download_dyld_file(device, i, filepath);
        CFArrayAppendValue(downloaded_files, download_path);
    }
    // extract files
    for (uint32_t i = 0; i < files_count; ++i) {
        CFStringRef download_path = (CFStringRef)CFArrayGetValueAtIndex(downloaded_files, i);
        dyld_shared_cache_extract_dylibs(dsc_extractor_bundle, download_path,
                                             symbols_download_directory);
        CFRelease(download_path);
    }

    CFRelease(downloaded_files);
    CFRelease(dsc_extractor_bundle);
}

typedef struct {
  uint32_t magic;
  uint32_t cb;
  uint16_t fragmentId;
  uint16_t fragmentCount;
  uint32_t length;
  uint32_t identifier;
  uint32_t conversationIndex;
  uint32_t channelCode;
  uint32_t expectsReply;
} DTXMessageHeader;

typedef struct {
  uint32_t flags;
  uint32_t auxiliaryLength;
  uint64_t totalLength;
} DTXMessagePayloadHeader;

uint32_t instruments_current_message_id = 0;

ServiceConnRef instrumentsServiceConnection = NULL;
NSDictionary<NSString*, NSNumber*>* instruments_available_channels = nil;

static const uint32 DTXMessageHeaderMagic = 0x1F3D5B79;
static const uint64 DTXAuxillaryDataMagic = 0x1F0;

static const uint32 EmptyDictionaryKey = 10;
static const uint32 ObjectArgumentType = 2;
static const uint32 Int32ArgumentType = 3;
static const uint32 Int64ArgumentType = 4;

NSData* instruments_object_argument(void * argument) {
    NSError *error = nil;
    NSData *argumentData = [NSKeyedArchiver archivedDataWithRootObject:argument];
    if (error) {
        on_error(@"Error communicating with the intruments server: %@", error);
    }
    uint32 argumentSize = (uint32) argumentData.length;
    NSMutableData *data = NSMutableData.data;
    [data appendBytes:&EmptyDictionaryKey length:sizeof(EmptyDictionaryKey)];
    [data appendBytes:&ObjectArgumentType length:sizeof(ObjectArgumentType)];
    [data appendBytes:&argumentSize length:sizeof(argumentSize)];
    [data appendData:argumentData];
    return data;
}

NSData* instruments_int32_argument(int32_t value) {
    NSMutableData *data = NSMutableData.data;
    [data appendBytes:&EmptyDictionaryKey length:sizeof(EmptyDictionaryKey)];
    [data appendBytes:&Int32ArgumentType length:sizeof(Int32ArgumentType)];
    [data appendBytes:&value length:sizeof(value)];
    return data;
}

NSData* instruments_int64_argument(int64_t value) {
    NSMutableData *data = NSMutableData.data;
    [data appendBytes:&EmptyDictionaryKey length:sizeof(EmptyDictionaryKey)];
    [data appendBytes:&Int64ArgumentType length:sizeof(Int64ArgumentType)];
    [data appendBytes:&value length:sizeof(value)];
    return data;
}

void instruments_send_message(int channel, NSString* selector, const NSArray<NSData*> *args, bool expects_reply) {
    uint32_t id = ++instruments_current_message_id;

    // Serialize arguments
    NSMutableData *auxillaryData = NSMutableData.data;
    if (args != nil) {
        NSMutableData *argumentsData = NSMutableData.data;
        for (NSData *argument in args) {
            [argumentsData appendData:argument];
        }

        uint64 payloadLength = argumentsData.length;
        [auxillaryData appendBytes:&DTXAuxillaryDataMagic length:sizeof(DTXAuxillaryDataMagic)];
        [auxillaryData appendBytes:&payloadLength length:sizeof(payloadLength)];
        [auxillaryData appendData:argumentsData];
    }

    // Serialize selector
    NSError *error = nil;
    NSData *selectorData = [NSKeyedArchiver archivedDataWithRootObject:selector];
    if (error) {
        on_error(@"Error communicating with the intruments server: %@", error);
    }

    // Prepare the message
    DTXMessagePayloadHeader payloadHeader;
    payloadHeader.flags = 0x2 | (expects_reply ? 0x1000 : 0);
    payloadHeader.auxiliaryLength = (uint32) auxillaryData.length;
    payloadHeader.totalLength = auxillaryData.length + selectorData.length;

    DTXMessageHeader messageHeader;
    messageHeader.magic = DTXMessageHeaderMagic;
    messageHeader.cb = sizeof(DTXMessageHeader);
    messageHeader.fragmentId = 0;
    messageHeader.fragmentCount = 1;
    messageHeader.length = (uint32_t)(sizeof(payloadHeader) + payloadHeader.totalLength);
    messageHeader.identifier = id;
    messageHeader.conversationIndex = 0;
    messageHeader.channelCode = channel;
    messageHeader.expectsReply = (expects_reply ? 1 : 0);

    NSMutableData *data = NSMutableData.data;
    [data appendBytes:&messageHeader length:sizeof(messageHeader)];
    [data appendBytes:&payloadHeader length:sizeof(payloadHeader)];
    [data appendData:auxillaryData];
    [data appendData:selectorData];

    // Send message
    uint64_t bytes_sent = AMDServiceConnectionSend(instrumentsServiceConnection, data.bytes, data.length);
    if (bytes_sent != data.length) {
        on_error(@"Error communicating with instruments server");
    }
}

NSArray<id>* instruments_parse_auxillary_data(NSData* data) {
    if (data == nil) {
        return nil;
    }

    if (data.length < 16) {
        on_error(@"Insufficient data to parse");
    }

    uint64_t offset = sizeof(uint64_t);

    uint64_t payloadLength;
    [data getBytes:&payloadLength range:NSMakeRange(offset, sizeof(payloadLength))];
    offset += sizeof(payloadLength);

    uint64_t end = offset + payloadLength;

    NSMutableArray<id> *arguments = NSMutableArray.array;

    while (offset < end) {
        uint32 type = 0;
        [data getBytes:&type range:NSMakeRange(offset, sizeof(type))];
        offset += sizeof(type);

        uint32 length = 0;

        id value = nil;

        switch (type) {
            case 2:
                [data getBytes:&length range:NSMakeRange(offset, sizeof(length))];
                offset += sizeof(length);
                value = [NSKeyedUnarchiver unarchiveObjectWithData:[data subdataWithRange:NSMakeRange(offset, length)]];
                break;
            case 3:
            case 5:
            {
                int32_t intValue;
                [data getBytes:&intValue range:NSMakeRange(offset, sizeof(intValue))];
                offset += sizeof(intValue);
                value = [NSNumber numberWithInt:intValue];
                break;
            }
            case 4:
            case 6:
            {
                int64_t intValue;
                [data getBytes:&intValue range:NSMakeRange(offset, sizeof(intValue))];
                offset += sizeof(intValue);
                value = [NSNumber numberWithLongLong:intValue];
                break;
            }
            case 10:
                // Empty dictionary value, ignore
                continue;
            default:
                // Unknown
                break;
        }

        if (value == nil) {
            on_error(@"Error communicating with instruments server: Error parsing auxiliary data");
        }
        [arguments addObject:value];
        offset += length;
    }

    return [arguments retain];
}

void instruments_receive_message(id* returnValue, NSArray<id>** auxillaryValues) {
    NSMutableData *payloadData = NSMutableData.data;

    DTXMessageHeader messageHeader;
    do {
        uint32_t bytes_read = AMDServiceConnectionReceive(instrumentsServiceConnection, &messageHeader, sizeof(messageHeader));
        if (bytes_read != sizeof(messageHeader)) {
            on_error(@"Error communicating with instruments server: Error in reading response");
        }
        if (messageHeader.magic != DTXMessageHeaderMagic) {
            on_error(@"Error communicating with instruments server: Magic in response magic does not match");
        }
        if (messageHeader.conversationIndex == 0 && messageHeader.identifier < instruments_current_message_id) {
            // New message with unexpected identifier
            on_error(@"Error communicating with instruments server: response identifier %d is lower than the last request (%d)", messageHeader.identifier, instruments_current_message_id);
        }
        if (messageHeader.conversationIndex == 1 && messageHeader.identifier != instruments_current_message_id) {
            // This is a response, but the message id does not match
            on_error(@"Error communicating with instruments server: expected response to message id %d, got %d", instruments_current_message_id, messageHeader.identifier);
        }

        instruments_current_message_id = messageHeader.identifier;

        if (messageHeader.fragmentId == 0 && messageHeader.fragmentCount > 1) {
            // First message in multi-fragment message has only payload.
            continue;
        }

        void *buffer = alloca(messageHeader.length);
        void *buffer_current = buffer;
        uint32_t remaining_bytes = messageHeader.length;
        while (remaining_bytes > 0) {
            uint32_t bytes_read = AMDServiceConnectionReceive(instrumentsServiceConnection, buffer_current, remaining_bytes);
            if (bytes_read <= 0) {
                on_error(@"Error communicating with instruments server: Error in reading response");
            }

            buffer_current += bytes_read;
            remaining_bytes -= bytes_read;
        }

        [payloadData appendBytes:buffer length:messageHeader.length];
    } while (messageHeader.fragmentId < messageHeader.fragmentCount - 1);

    const DTXMessagePayloadHeader *payloadHeader = (const DTXMessagePayloadHeader *)payloadData.bytes;

    if ((payloadHeader->flags & 0xFF000) >> 12) {
        on_error(@"Error communicating with instruments server: Compression is not supported");
    }

    // serialized object array is located just after payload header
    NSData* auxillaryData = nil;
    if (payloadHeader->auxiliaryLength) {
        auxillaryData = [payloadData subdataWithRange:NSMakeRange(sizeof(DTXMessagePayloadHeader), payloadHeader->auxiliaryLength)];
    }
    if (auxillaryValues != nil) {
        *auxillaryValues = instruments_parse_auxillary_data(auxillaryData);
    }

    // archived payload object appears after the auxiliary array
    size_t returnValueDataLength = payloadHeader->totalLength - payloadHeader->auxiliaryLength;
    NSData* returnValueData = nil;
    if (returnValueDataLength) {
        returnValueData = [payloadData subdataWithRange:NSMakeRange(sizeof(DTXMessagePayloadHeader) + payloadHeader->auxiliaryLength, returnValueDataLength)];
    }
    if (returnValue != nil) {
        *returnValue = [NSKeyedUnarchiver unarchiveObjectWithData:returnValueData];
    }
}

void instruments_perform_handshake() {
    NSDictionary* capabilities = @{
        @"com.apple.private.DTXBlockCompression": @2,
        @"com.apple.private.DTXConnection": @1
    };

    instruments_send_message(
        0 /* channel */,
        @"_notifyOfPublishedCapabilities:",
        @[
            instruments_object_argument(capabilities)
        ] /* args */,
        false /* expectes_reply */
    );

    id returnValue = nil;
    NSArray<id>* auxillaryValues = nil;
    instruments_receive_message(&returnValue, &auxillaryValues);

    if (![returnValue isKindOfClass:NSString.class] || ![returnValue isEqualToString:@"_notifyOfPublishedCapabilities:"]) {
        on_error(@"Error communicating with instruments server: unexpected response selector");
    }

    NSDictionary<NSString*, NSNumber*>* channels = auxillaryValues.firstObject;
    if (![channels isKindOfClass:NSDictionary.class]) {
        on_error(@"Error communicating with instruments server: unexpected channel list type");
    }

    instruments_available_channels = [channels retain];

    [returnValue release];
    [auxillaryValues release];
}

id instruments_perform_selector(int channel, NSString* selector, const NSArray<NSData*> *args) {
    instruments_send_message(channel, selector, args, true /* expectes_reply */);

    id returnValue = nil;
    instruments_receive_message(&returnValue, nil);

    return returnValue;
}

int32_t instruments_make_channel(NSString* identifier) {
    if (![instruments_available_channels objectForKey:identifier]) {
        on_error(@"Channel %@ not supported by the server", identifier);
    }

    static int32_t channel_id = 0;
    int32_t code = ++channel_id;

    id returnValue = instruments_perform_selector(
        0 /* channel */,
        @"_requestChannelWithCode:identifier:",
        @[
            instruments_int32_argument(code),
            instruments_object_argument(identifier)
        ]
    );

    if (returnValue != nil) {
        on_error(@"Error: _requestChannelWithCode:identifier: returned %@", returnValue);
    }

    [returnValue release];

    return code;
}

void instruments_connect_service(AMDeviceRef device) {
    connect_and_start_session(device);
    mount_developer_image(device);

    // Check version similar to start_remote_debug_server
    CFStringRef keys[] = { CFSTR("MinIPhoneVersion"), CFSTR("MinAppleTVVersion"), CFSTR("MinWatchVersion") };
    CFStringRef values[] = { CFSTR("14.0"), CFSTR("14.0"), CFSTR("7.0") }; // Not sure about older watchOS versions
    CFDictionaryRef version = CFDictionaryCreate(NULL, (const void **)&keys, (const void **)&values, 3, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

    bool useSecureProxy = AMDeviceIsAtLeastVersionOnPlatform(device, version);

    // Start the instruments server
    assert(instrumentsServiceConnection == NULL);
    CFStringRef serviceName = useSecureProxy ? CFSTR("com.apple.instruments.remoteserver.DVTSecureSocketProxy") : CFSTR("com.apple.instruments.remoteserver");
    check_error(AMDeviceSecureStartService(device, serviceName, NULL, &instrumentsServiceConnection));

    assert(instrumentsServiceConnection != NULL);

    if (!useSecureProxy)
    {
        disable_ssl(instrumentsServiceConnection);
    }

    check_error(AMDeviceStopSession(device));
    check_error(AMDeviceDisconnect(device));
}

NSNumber* pid_for_bundle_id(NSString* bundle_id) {
    int32_t channel = instruments_make_channel(@"com.apple.instruments.server.services.processcontrol");

    id pid = instruments_perform_selector(channel, @"processIdentifierForBundleIdentifier:", @[instruments_object_argument(bundle_id)]);

    if (pid != nil && ![pid isKindOfClass:NSNumber.class]) {
        on_error(@"Error: did not get valid response from processIdentifierForBundleIdentifier:");
    }

    // Return -1 if pid is not found
    return (pid == nil || [pid isEqualToNumber:@0]) ? @-1 : pid;
}

void get_pid(AMDeviceRef device) {
    if (bundle_id == NULL) {
        on_error(@"Error: bundle id required, please specify with --bundle_id.");
    }

    instruments_connect_service(device);
    instruments_perform_handshake();

    CFStringRef cf_bundle_id = CFAutorelease(CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8));

    NSNumber* pid = pid_for_bundle_id((NSString*)cf_bundle_id);

    NSLogOut(@"pid: %@", pid);
    NSLogJSON(@{@"Event": @"GetPid",
                @"pid": pid});
}

void kill_app(AMDeviceRef device) {
    if (bundle_id == NULL && command_pid <= 0) {
        on_error(@"Error: must specify either --pid or --bundle_id");
    }

    instruments_connect_service(device);
    instruments_perform_handshake();

    NSNumber* ns_pid = [NSNumber numberWithInt:command_pid];
    if (![ns_pid isGreaterThan:@0]) {
        CFStringRef cf_bundle_id = CFAutorelease(CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8));
        ns_pid = pid_for_bundle_id((NSString*)cf_bundle_id);

        if (![ns_pid isGreaterThan:@0]) {
            NSLogOut(@"Could not find pid for bundle '%@'. Nothing to kill.", cf_bundle_id);
            return;
        }
    }

    int32_t channel = instruments_make_channel(@"com.apple.instruments.server.services.processcontrol");

    instruments_send_message(channel, @"killPid:", @[instruments_object_argument(ns_pid)], false /* expectes_reply */);
    [ns_pid release];
}

void list_processes(AMDeviceRef device) {
    instruments_connect_service(device);
    instruments_perform_handshake();

    int32_t channel = instruments_make_channel(@"com.apple.instruments.server.services.deviceinfo");

    id processes = instruments_perform_selector(channel, @"runningProcesses", nil /* args */);

    if (processes == nil || ![processes isKindOfClass:NSArray.class]) {
        on_error(@"Error: could not retrieve return value for runningProcesses");
    }

    if (bundle_id != NULL) {
        CFStringRef cf_bundle_id = CFAutorelease(CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8));
        NSNumber* pid = pid_for_bundle_id((NSString*)cf_bundle_id);

        NSMutableArray* filteredProcesses = NSMutableArray.array;

        if ([pid isGreaterThan:@0]) {
            for (NSDictionary* proc in processes) {
                NSNumber* procPid = proc[@"pid"];
                if (procPid == pid) {
                    [filteredProcesses addObject:proc];
                }
            }
        }

        [processes release];
        processes = filteredProcesses;
    }

    if (_json_output) {
        // NSDate cannot be serialized to JSON as is, manually convert it it NSString
        NSDateFormatter *dateFormatter = [[NSDateFormatter alloc] init];
        [dateFormatter setDateFormat:@"yyyy'-'MM'-'dd'T'HH':'mm':'ss.SSS'Z'"];
        [dateFormatter setTimeZone:[NSTimeZone timeZoneWithName:@"GMT"]];

        NSMutableArray* processesCopy = NSMutableArray.array;
        for (NSDictionary* proc in processes) {
            NSMutableDictionary* procCopy = [NSMutableDictionary dictionaryWithDictionary:proc];

            if (procCopy[@"startDate"] != nil) {
                NSString *startDate = [dateFormatter stringFromDate:procCopy[@"startDate"]];
                [procCopy removeObjectForKey:@"startDate"];
                [procCopy setObject:startDate forKey:@"startDate"];
            }

            [processesCopy addObject:procCopy];
        }

        NSLogJSON(@{@"Event": @"ListProcesses",
                    @"Processes": processesCopy});
    }
    else {
        NSLogOut(@"PID\tNAME");
        for (NSDictionary* proc in processes) {
            NSLogOut(@"%@\t%@", proc[@"pid"], proc[@"name"]);
        }
    }

    [processes release];
}

void handle_device(AMDeviceRef device) {
    NSLogVerbose(@"Already found device? %d", found_device);

    CFStringRef device_full_name = get_device_full_name(device),
                device_interface_name = get_device_interface_name(device);

    if (detect_only) {
        if (_json_output) {
            NSLogJSON(@{@"Event": @"DeviceDetected",
                        @"Interface": (__bridge NSString *)device_interface_name,
                        @"Device": get_device_json_dict(device)
                        });
        } else {
            NSLogOut(@"[....] Found %@ connected through %@.", device_full_name, device_interface_name);
        }
        found_device = true;
        return;
    }
    if (found_device)
    {
        NSLogVerbose(@"Skipping %@.", device_full_name);
        return;
    }
    CFStringRef found_device_id = CFAutorelease(AMDeviceCopyDeviceIdentifier(device));
    if (device_id != NULL) {
        CFStringRef deviceCFSTR = CFAutorelease(CFStringCreateWithCString(NULL, device_id, kCFStringEncodingUTF8));
        if (CFStringCompare(deviceCFSTR, found_device_id, kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
            found_device = true;
        } else {
            NSLogVerbose(@"Skipping %@.", device_full_name);
            return;
        }
    } else {
        // Use the first device we find if a device_id wasn't specified.
        device_id = strdup(CFStringGetCStringPtr(found_device_id, kCFStringEncodingUTF8));
        found_device = true;
    }

    NSLogOut(@"[....] Using %@.", device_full_name);

    if (command_only) {
        if (strcmp("list", command) == 0) {
            list_files(device);
        } else if (strcmp("upload", command) == 0) {
            upload_file(device);
        } else if (strcmp("download", command) == 0) {
            download_tree(device);
        } else if (strcmp("mkdir", command) == 0) {
            make_directory(device);
        } else if (strcmp("rm", command) == 0) {
            remove_path(device);
        } else if (strcmp("rmtree", command) == 0) {
            rmtree(device);
        } else if (strcmp("exists", command) == 0) {
            exit(app_exists(device));
        } else if (strcmp("uninstall_only", command) == 0) {
            uninstall_app(device);
        } else if (strcmp("list_bundle_id", command) == 0) {
            list_bundle_id(device);
        } else if (strcmp("list_processes", command) == 0) {
            list_processes(device);
        } else if (strcmp("get_pid", command) == 0) {
            get_pid(device);
        } else if (strcmp("get_battery_level", command) == 0) {
            get_battery_level(device);
        } else if (strcmp("symbols", command) == 0) {
            download_device_symbols(device);
        } else if (strcmp("list_profiles", command) == 0) {
            list_provisioning_profiles(device);
        } else if (strcmp("install_profile", command) == 0) {
            install_provisioning_profile(device);
        } else if (strcmp("uninstall_profile", command) == 0) {
            uninstall_provisioning_profile(device);
        } else if (strcmp("download_profile", command) == 0) {
            download_provisioning_profile(device);
#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
        } else if (strcmp("check_developer_mode", command) == 0) {
            check_developer_mode(device);
#endif
        } else if (strcmp("kill_app", command) == 0) {
            kill_app(device);
        }
        exit(0);
    }

    if (debugserver_only && app_path == NULL) {
        launch_debugserver_only(device, NULL);
        return;
    }

    CFRetain(device); // don't know if this is necessary?

    CFStringRef path = CFStringCreateWithCString(NULL, app_path, kCFStringEncodingUTF8);
    CFURLRef relative_url = CFURLCreateWithFileSystemPath(NULL, path, kCFURLPOSIXPathStyle, false);
    CFURLRef url = CFURLCopyAbsoluteURL(relative_url);

    CFRelease(relative_url);

    if (uninstall) {
        NSLogOut(@"------ Uninstall phase ------");

        //Do we already have the bundle_id passed in via the command line? if so, use it.
        CFStringRef cf_uninstall_bundle_id = NULL;
        if (bundle_id != NULL)
        {
            cf_uninstall_bundle_id = CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8);
        } else {
            cf_uninstall_bundle_id = copy_bundle_id(url);
        }

        if (cf_uninstall_bundle_id == NULL) {
            on_error(@"Error: Unable to get bundle id from user command or package %@.\nUninstall failed.", [NSString stringWithUTF8String:app_path]);
        } else {
            connect_and_start_session(device);

            int code = AMDeviceSecureUninstallApplication(0, device, cf_uninstall_bundle_id, 0, NULL, 0);
            if (code == 0) {
                NSLogOut(@"[ OK ] Uninstalled package with bundle id %@", cf_uninstall_bundle_id);
            } else {
                on_error(@"[ ERROR ] Could not uninstall package with bundle id %@", cf_uninstall_bundle_id);
            }
            CFRelease(cf_uninstall_bundle_id);
            check_error(AMDeviceStopSession(device));
            check_error(AMDeviceDisconnect(device));
        }
    }

    if(install) {
        NSLogOut(@"------ Install phase ------");
        NSLogOut(@"[  0%%] Found %@ connected through %@, beginning install", device_full_name, device_interface_name);

        CFStringRef install_bundle_id = bundle_id == NULL ? copy_bundle_id(url) : CFStringCreateWithCString(NULL, bundle_id, kCFStringEncodingUTF8);

        CFDictionaryRef options;
        if (app_deltas == NULL) { // standard install
          CFStringRef keys[] = { CFSTR("PackageType") };
          CFStringRef values[] = { CFSTR("Developer") };
          options = CFDictionaryCreate(NULL, (const void **)&keys, (const void **)&values, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
          check_error(AMDeviceSecureTransferPath(0, device, url, options, transfer_callback, 0));

          connect_and_start_session(device);
          check_error(AMDeviceSecureInstallApplication(0, device, url, options, install_callback, 0));
          check_error(AMDeviceStopSession(device));
          check_error(AMDeviceDisconnect(device));
        } else { // incremental install
          if (install_bundle_id == NULL) {
            on_error(@"[ ERROR] Could not determine bundle id.");
          }
          CFStringRef deltas_path =
            CFStringCreateWithCString(NULL, app_deltas, kCFStringEncodingUTF8);
          CFURLRef deltas_relative_url =
            CFURLCreateWithFileSystemPath(NULL, deltas_path, kCFURLPOSIXPathStyle, false);
          CFURLRef app_deltas_url = CFURLCopyAbsoluteURL(deltas_relative_url);
          CFStringRef prefer_wifi = no_wifi ? CFSTR("0") : CFSTR("1");

          // These values were determined by inspecting Xcode 11.1 logs with the Console app.
          CFStringRef keys[] = {
            CFSTR("CFBundleIdentifier"),
            CFSTR("CloseOnInvalidate"),
            CFSTR("InvalidateOnDetach"),
            CFSTR("IsUserInitiated"),
            CFSTR("PackageType"),
            CFSTR("PreferWifi"),
            CFSTR("ShadowParentKey"),
          };
          CFStringRef values[] = {
            install_bundle_id,
            CFSTR("1"),
            CFSTR("1"),
            CFSTR("1"),
            CFSTR("Developer"),
            prefer_wifi,
            (CFStringRef)app_deltas_url,
          };

          CFIndex size = sizeof(keys)/sizeof(CFStringRef);
          options = CFDictionaryCreate(NULL, (const void **)&keys, (const void **)&values, size, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

          // Incremental installs should be done without a session started because of timeouts.
          check_error(AMDeviceSecureInstallApplicationBundle(device, url, options, incremental_install_callback, 0));
          CFRelease(deltas_path);
          CFRelease(deltas_relative_url);
          CFRelease(app_deltas_url);
          free(app_deltas);
          app_deltas = NULL;
        }

        CFRelease(options);

        NSLogOut(@"[100%%] Installed package %@", [NSString stringWithUTF8String:app_path]);
        if (install_bundle_id == NULL) {
          NSLogJSON(@{@"Event": @"BundleInstall",
                      @"OverallPercent": @(100),
                      @"Percent": @(100),
                      @"Status": @"Complete"
                      });
        } else {
          connect_and_start_session(device);
          CFURLRef device_app_url = copy_device_app_url(device, install_bundle_id);
          check_error(AMDeviceStopSession(device));
          check_error(AMDeviceDisconnect(device));
          CFStringRef device_app_path = CFURLCopyFileSystemPath(device_app_url, kCFURLPOSIXPathStyle);
          
          NSLogVerbose(@"App path: %@", device_app_path);
          NSLogJSON(@{@"Event": @"BundleInstall",
                      @"OverallPercent": @(100),
                      @"Percent": @(100),
                      @"Status": @"Complete",
                      @"Path": (__bridge NSString *)device_app_path
                      });

          CFRelease(device_app_url);
          CFRelease(install_bundle_id);
          CFRelease(device_app_path);
        }
    }
    CFRelease(path);

    if (!debug)
        exit(0); // no debug phase

    if (justlaunch) {
        launch_debugger_and_exit(device, url);
    } else if (debugserver_only) {
        launch_debugserver_only(device, url);
    } else {
        launch_debugger(device, url);
    }
}

void log_device_disconnected(AMDeviceRef device) {
    CFStringRef device_interface_name = get_device_interface_name(device);
    CFStringRef device_uuid = CFAutorelease(AMDeviceCopyDeviceIdentifier(device));

    if (_json_output) {
        NSLogJSON(@{@"Event": @"DeviceDisconnected",
                    @"Interface": (__bridge NSString *)device_interface_name,
                    @"Device": get_device_json_dict(device)
                    });
    }
    else {
        NSLogOut(@"[....] Disconnected %@ from %@.", device_uuid, device_interface_name);
    }
}

void handle_device_disconnected(AMDeviceRef device) {
    CFStringRef device_interface_name = get_device_interface_name(device);
    CFStringRef device_uuid = AMDeviceCopyDeviceIdentifier(device);

    if (detect_only) {
        log_device_disconnected(device);
    }
    else {
        NSLogVerbose(@"[....] Disconnected %@ from %@.", device_uuid, device_interface_name);
    }

    if (debugserver_only) {
        CFStringRef deviceCFSTR = CFAutorelease(CFStringCreateWithCString(NULL, device_id, kCFStringEncodingUTF8));
        if (CFStringCompare(deviceCFSTR, device_uuid, kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
            log_device_disconnected(device);
            exit(0);
        }
    }

    CFRelease(device_uuid);
}

void device_callback(struct am_device_notification_callback_info *info, void *arg) {
    switch (info->msg) {
        case ADNCI_MSG_CONNECTED:
        {
            int itype = AMDeviceGetInterfaceType(info->dev);
            if (no_wifi &&  (itype == 2 || ( itype == 3 && get_companion_interface_type(info->dev) == 2)))
            {
                NSLogVerbose(@"Skipping wifi device (type: %d)", itype);
            }
            else
            {
                NSLogVerbose(@"Handling device type: %d", itype);
                handle_device(info->dev);
            }
            break;
        }
        case ADNCI_MSG_DISCONNECTED:
        {
            handle_device_disconnected(info->dev);
            break;
        }
        default:
            break;
    }
}

void timeout_callback(CFRunLoopTimerRef timer, void *info) {
    if (found_device && (!detect_only)) {
        // Don't need to exit in the justlaunch mode
        if (justlaunch)
            return;

        // App running for too long
        NSLogOut(@"[ !! ] App is running for too long");
        exit(exitcode_timeout);
        return;
    } else if ((!found_device) && (!detect_only))  {
        on_error(@"Timed out waiting for device.");
    }
    else
    {
        // Device detection timeout
        if (!debug) {
            NSLogOut(@"[....] No more devices found.");
        }

        if (detect_only && !found_device) {
            exit(exitcode_error);
            return;
        } else {
            int mypid = getpid();
            if ((parent != 0) && (parent == mypid) && (child != 0))
            {
                NSLogVerbose(@"Timeout. Killing child (%d) tree.", child);
                kill_ptree(child, SIGHUP);
            }
        }
        exit(0);
    }
}

void usage(const char* app) {
    NSLog(
        @"Usage: %@ [OPTION]...\n"
        @"  -d, --debug                  launch the app in lldb after installation\n"
        @"  -i, --id <device_id>         the id of the device to connect to\n"
        @"  -c, --detect                 list all connected devices\n"
        @"  -b, --bundle <bundle.app>    the path to the app bundle to be installed\n"
        @"  -a, --args <args>            command line arguments to pass to the app when launching it\n"
        @"  -s, --envs <envs>            environment variables, space separated key-value pairs, to pass to the app when launching it\n"
        @"  -t, --timeout <timeout>      number of seconds to wait for a device to be connected\n"
        @"  -u, --unbuffered             don't buffer stdout\n"
        @"  -n, --nostart                do not start the app when debugging\n"
        @"  -N, --nolldb                 start debugserver only. do not run lldb. Can not be used with args or envs options\n"
        @"  -I, --noninteractive         start in non interactive mode (quit when app crashes or exits)\n"
        @"  -L, --justlaunch             just launch the app and exit lldb\n"
        @"  -v, --verbose                enable verbose output\n"
        @"  -m, --noinstall              directly start debugging without app install (-d not required)\n"
        @"  -A, --app_deltas             incremental install. must specify a directory to store app deltas to determine what needs to be installed\n"
        @"  -p, --port <number>          port used for device, default: dynamic\n"
        @"  -r, --uninstall              uninstall the app before install (do not use with -m; app cache and data are cleared) \n"
        @"  -9, --uninstall_only         uninstall the app ONLY. Use only with -1 <bundle_id> \n"
        @"  -1, --bundle_id <bundle id>  specify bundle id for list and upload\n"
        @"  -l, --list[=<dir>]           list all app files or the specified directory\n"
        @"  -o, --upload <file>          upload file\n"
        @"  -w, --download[=<path>]      download app tree or the specified file/directory\n"
        @"  -2, --to <target pathname>   use together with up/download file/tree. specify target\n"
        @"  -D, --mkdir <dir>            make directory on device\n"
        @"  -R, --rm <path>              remove file or directory on device (directories must be empty)\n"
        @"  -X, --rmtree <path>          remove directory and all contained files recursively on device\n"
        @"  -V, --version                print the executable version \n"
        @"  -e, --exists                 check if the app with given bundle_id is installed or not \n"
        @"  -B, --list_bundle_id         list bundle_id \n"
        @"  --list_processes             list running processes \n"
        @"  --get_pid                    get process id for the bundle. must specify --bundle_id\n"
        @"  --pid <pid>                  specify pid, to be used with --kill\n"
        @"  --kill                       kill a process. must specify either --pid or --bundle_id\n"
        @"  -W, --no-wifi                ignore wifi devices\n"
        @"  -C, --get_battery_level      get battery current capacity \n"
        @"  -O, --output <file>          write stdout to this file\n"
        @"  -E, --error_output <file>    write stderr to this file\n"
        @"  --detect_deadlocks <sec>     start printing backtraces for all threads periodically after specific amount of seconds\n"
        @"  -f, --file_system            specify file system for mkdir / list / upload / download / rm\n"
        @"  -F, --non-recursively        specify non-recursively walk directory\n"
        @"  -S, --symbols                download OS symbols. must specify a directory to store the downloaded symbols\n"
        @"  -j, --json                   format output as JSON\n"
        @"  -k, --key                    keys for the properties of the bundle. Joined by ',' and used only with -B <list_bundle_id> and -j <json> \n"
        @"  --custom-script <script>     path to custom python script to execute in lldb\n"
        @"  --custom-command <command>   specify additional lldb commands to execute\n"
        @"  --faster-path-search         use alternative logic to find the device support paths faster\n"
        @"  -P, --list_profiles          list all provisioning profiles on device\n"
        @"  --profile-uuid <uuid>        the UUID of the provisioning profile to target, use with other profile commands\n"
        @"  --profile-download <path>    download a provisioning profile (requires --profile-uuid)\n"
        @"  --profile-install <file>     install a provisioning profile\n"
        @"  --profile-uninstall          uninstall a provisioning profile (requires --profile-uuid <UUID>)\n"
#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
        @"  --check-developer-mode       checks whether the given device has developer mode enabled (requires Xcode 14 or newer)\n"
#endif
        ,
        [NSString stringWithUTF8String:app]);
}

void show_version(void) {
    NSLogOut(@"%@", @
#include "version.h"
             );
}

int main(int argc, char *argv[]) {

    // create a UUID for tmp purposes
    CFUUIDRef uuid = CFUUIDCreate(NULL);
    CFStringRef str = CFUUIDCreateString(NULL, uuid);
    CFRelease(uuid);
    tmpUUID = [(NSString*)str autorelease];

    static struct option longopts[] = {
        { "debug", no_argument, NULL, 'd' },
        { "id", required_argument, NULL, 'i' },
        { "bundle", required_argument, NULL, 'b' },
        { "args", required_argument, NULL, 'a' },
        { "envs", required_argument, NULL, 's' },
        { "verbose", no_argument, NULL, 'v' },
        { "timeout", required_argument, NULL, 't' },
        { "unbuffered", no_argument, NULL, 'u' },
        { "nostart", no_argument, NULL, 'n' },
        { "nolldb", no_argument, NULL, 'N' },
        { "noninteractive", no_argument, NULL, 'I' },
        { "justlaunch", no_argument, NULL, 'L' },
        { "detect", no_argument, NULL, 'c' },
        { "version", no_argument, NULL, 'V' },
        { "noinstall", no_argument, NULL, 'm' },
        { "port", required_argument, NULL, 'p' },
        { "uninstall", no_argument, NULL, 'r' },
        { "uninstall_only", no_argument, NULL, '9'},
        { "list", optional_argument, NULL, 'l' },
        { "bundle_id", required_argument, NULL, '1'},
        { "upload", required_argument, NULL, 'o'},
        { "download", optional_argument, NULL, 'w'},
        { "to", required_argument, NULL, '2'},
        { "mkdir", required_argument, NULL, 'D'},
        { "rm", required_argument, NULL, 'R'},
        { "rmtree",required_argument, NULL, 'X'},
        { "exists", no_argument, NULL, 'e'},
        { "list_bundle_id", no_argument, NULL, 'B'},
        { "no-wifi", no_argument, NULL, 'W'},
        { "get_battery_level", no_argument, NULL, 'C'},
        { "output", required_argument, NULL, 'O' },
        { "error_output", required_argument, NULL, 'E' },
        { "detect_deadlocks", required_argument, NULL, 1000 },
        { "json", no_argument, NULL, 'j'},
        { "app_deltas", required_argument, NULL, 'A'},
        { "file_system", no_argument, NULL, 'f'},
        { "non-recursively", no_argument, NULL, 'F'},
        { "key", optional_argument, NULL, 'k' },
        { "symbols", required_argument, NULL, 'S' },
        { "list_profiles", no_argument, NULL, 'P' },
        { "custom-script", required_argument, NULL, 1001},
        { "custom-command", required_argument, NULL, 1002},
        { "faster-path-search", no_argument, NULL, 1003},
        { "profile-install", required_argument, NULL, 1004},
        { "profile-uninstall", no_argument, NULL, 1005},
        { "profile-download", required_argument, NULL, 1006},
        { "profile-uuid", required_argument, NULL, 1007},
#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
        { "check-developer-mode", no_argument, NULL, 1008},
#endif
        { "list_processes", no_argument, NULL, 1009},
        { "get_pid", no_argument, NULL, 1010},
        { "pid", required_argument, NULL, 1011},
        { "kill", no_argument, NULL, 1012},
        { NULL, 0, NULL, 0 },
    };
    int ch;

    while ((ch = getopt_long(argc, argv, "VmcdvunrILefFD:R:X:i:b:a:t:p:1:2:o:l:w:9BWjNs:OE:CA:k:S:P", longopts, NULL)) != -1)
    {
        switch (ch) {
        case 'm':
            install = false;
            debug = true;
            break;
        case 'd':
            debug = true;
            break;
        case 'i':
            device_id = optarg;
            break;
        case 'b':
            app_path = optarg;
            break;
        case 'a':
            args = optarg;
            break;
        case 's':
            envs = optarg;
            break;
        case 'S':
            symbols_download_directory = optarg;
            command = "symbols";
            command_only = true;
            break;
        case 'v':
            verbose = true;
            break;
        case 't':
            _timeout = atoi(optarg);
            break;
        case 'u':
            unbuffered = true;
            break;
        case 'n':
            nostart = true;
            break;
        case 'N':
            debugserver_only = true;
            debug = true;
            break;
        case 'I':
            interactive = false;
            debug = true;
            break;
        case 'L':
            interactive = false;
            justlaunch = true;
            debug = true;
            break;
        case 'c':
            detect_only = true;
            debug = true;
            break;
        case 'V':
            show_version();
            return 0;
        case 'p':
            port = atoi(optarg);
            break;
        case 'r':
            uninstall = true;
            break;
        case '9':
            command_only = true;
            command = "uninstall_only";
            break;
        case '1':
            bundle_id = optarg;
            break;
        case '2':
            target_filename = optarg;
            break;
        case 'o':
            command_only = true;
            upload_pathname = optarg;
            command = "upload";
            break;
        case 'l':
            command_only = true;
            command = "list";
            list_root = optarg;
            break;
        case 'w':
            command_only = true;
            command = "download";
            list_root = optarg;
            break;
        case 'D':
            command_only = true;
            target_filename = optarg;
            command = "mkdir";
            break;
        case 'R':
            command_only = true;
            target_filename = optarg;
            command = "rm";
            break;
        case 'X':
            command_only = true;
            target_filename = optarg;
            command = "rmtree";
            break;
        case 'e':
            command_only = true;
            command = "exists";
            break;
        case 'B':
            command_only = true;
            command = "list_bundle_id";
            break;
        case 1009:
            command_only = true;
            command = "list_processes";
            break;
        case 1010:
            command_only = true;
            command = "get_pid";
            break;
        case 'W':
            no_wifi = true;
            break;
        case 'C':
            command_only = true;
            command = "get_battery_level";
            break;
        case 'O':
            output_path = optarg;
            break;
        case 'E':
            error_path = optarg;
            break;
        case 1000:
            _detectDeadlockTimeout = atoi(optarg);
            break;
        case 'j':
            _json_output = true;
            break;
        case 'A':
            app_deltas = resolve_path(optarg);
            break;
        case 'f':
            file_system = true;
            break;
        case 'F':
            non_recursively = true;
            break;
        case 1001:
            custom_script_path = optarg;
            break;
        case 1002:
            if (custom_commands == nil)
            {
                custom_commands = [[NSMutableString alloc] init];
            }
            [custom_commands appendFormat:@"%s\n", optarg];
            break;
        case 1003:
            faster_path_search = true;
            break;
        case 1004:
            command_only = true;
            command = "install_profile";
            profile_path = optarg;
            break;
        case 1005:
            command_only = true;
            command = "uninstall_profile";
            break;
        case 1006:
            command_only = true;
            command = "download_profile";
            profile_path = optarg;
            break;
        case 1007:
            profile_uuid = optarg;
            break;
#if defined(IOS_DEPLOY_FEATURE_DEVELOPER_MODE)
        case 1008:
          command_only = true;
          command = "check_developer_mode";
          break;
#endif
        case 'P':
            command_only = true;
            command = "list_profiles";
            break;
        case 'k':
            if (!keys) keys = [[NSMutableArray alloc] init];
            [keys addObject: [NSString stringWithUTF8String:optarg]];
            break;
        case 1011:
            command_pid = atoi(optarg);
            break;
        case 1012:
            command_only = true;
            command = "kill_app";
            break;
        default:
            usage(argv[0]);
            return exitcode_error;
        }
    }
    
    if (debugserver_only && (args || envs)) {
        usage(argv[0]);
        on_error(@"The --args and --envs options can not be combined with --nolldb.");
    }

    if (!app_path && !detect_only && !debugserver_only && !command_only) {
        usage(argv[0]);
        on_error(@"One of -[b|c|o|l|w|D|N|R|X|e|B|C|S|9] is required to proceed!");
    }

    if (unbuffered) {
        setbuf(stdout, NULL);
        setbuf(stderr, NULL);
    }

    if (detect_only && _timeout == 0) {
        _timeout = 5;
    }

    if (app_path) {
        if (access(app_path, F_OK) != 0) {
            on_sys_error(@"Can't access app path '%@'", [NSString stringWithUTF8String:app_path]);
        }
    }

    AMDSetLogLevel(5); // otherwise syslog gets flooded with crap
    if (_timeout > 0)
    {
        CFRunLoopTimerRef timer = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + _timeout, 0, 0, 0, timeout_callback, NULL);
        CFRunLoopAddTimer(CFRunLoopGetCurrent(), timer, kCFRunLoopCommonModes);
        NSLogOut(@"[....] Waiting up to %d seconds for iOS device to be connected", _timeout);
    }
    else
    {
        NSLogOut(@"[....] Waiting for iOS device to be connected");
    }


    CFStringRef keys[] = {
        CFSTR("NotificationOptionSearchForPairedDevices"),
    };
    const void* values[] = {
        kCFBooleanTrue,
    };
    CFDictionaryRef options = CFDictionaryCreate(kCFAllocatorDefault, (const void**)keys, values, 1, &kCFCopyStringDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);

    AMDeviceNotificationSubscribeWithOptions(&device_callback, 0, 0, NULL, &notify, options);
    CFRunLoopRun();
}
