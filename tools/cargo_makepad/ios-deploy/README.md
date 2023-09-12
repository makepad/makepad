[![Build Status](https://travis-ci.org/ios-control/ios-deploy.svg?branch=master)](https://travis-ci.org/ios-control/ios-deploy)

ios-deploy
==========

Install and debug iOS apps from the command line. Designed to work on un-jailbroken devices.

## Requirements

* macOS
* You need to have a valid iOS Development certificate installed
* Xcode (**NOT** just Command Line Tools!)

#### Tested Configurations
The ios-deploy binary in Homebrew should work on macOS 10.0+ with Xcode7+. It has been most recently tested with the following configurations:
 - macOS 10.14 Mojave, 10.15 Catalina and preliminary testing on 11.0b BigSur
 - iOS 13.0 and preliminary testing on iOS 14.0b
 - Xcode 11.3, 11.6 and preliminary testing on Xcode 12 betas
 - x86 and preliminary testing on Arm64e based Apple Macintosh Computers

## Roadmap

See our [milestones](https://github.com/phonegap/ios-deploy/milestones).
	
## Development

The 1.x branch has been archived (renamed for now), all development is to be on the master branch for simplicity, since the planned 2.x development (break out commands into their own files) has been abandoned for now.

## Installation

If you have previously installed ios-deploy via `npm`, uninstall it by running:
```
sudo npm uninstall -g ios-deploy
```

Install ios-deploy via [Homebrew](https://brew.sh/) by running:

```
brew install ios-deploy
```

## Testing

Run:

```
python -m py_compile src/scripts/*.py && xcodebuild -target ios-deploy && xcodebuild test -scheme ios-deploy-tests
```

## Usage

    Usage: ios-deploy [OPTION]...
	  -d, --debug                  launch the app in lldb after installation
	  -i, --id <device_id>         the id of the device to connect to
	  -c, --detect                 list all connected devices
	  -b, --bundle <bundle.app>    the path to the app bundle to be installed
	  -a, --args <args>            command line arguments to pass to the app when launching it
	  -s, --envs <envs>            environment variables, space separated key-value pairs, to pass to the app when launching it
	  -t, --timeout <timeout>      number of seconds to wait for a device to be connected
	  -u, --unbuffered             don't buffer stdout
	  -n, --nostart                do not start the app when debugging
	  -N, --nolldb                 start debugserver only. do not run lldb. Can not be used with args or envs options
	  -I, --noninteractive         start in non interactive mode (quit when app crashes or exits)
	  -L, --justlaunch             just launch the app and exit lldb
	  -v, --verbose                enable verbose output
	  -m, --noinstall              directly start debugging without app install (-d not required)
	  -A, --app_deltas             incremental install. must specify a directory to store app deltas to determine what needs to be installed
	  -p, --port <number>          port used for device, default: dynamic
	  -r, --uninstall              uninstall the app before install (do not use with -m; app cache and data are cleared)
	  -9, --uninstall_only         uninstall the app ONLY. Use only with -1 <bundle_id>
	  -1, --bundle_id <bundle id>  specify bundle id for list and upload
	  -l, --list[=<dir>]           list all app files or the specified directory
	  -o, --upload <file>          upload file
	  -w, --download[=<path>]      download app tree or the specified file/directory
	  -2, --to <target pathname>   use together with up/download file/tree. specify target
	  -D, --mkdir <dir>            make directory on device
	  -R, --rm <path>              remove file or directory on device (directories must be empty)
	  -X, --rmtree <path>          remove directory and all contained files recursively on device
	  -V, --version                print the executable version
	  -e, --exists                 check if the app with given bundle_id is installed or not
	  -B, --list_bundle_id         list bundle_id
	  -W, --no-wifi                ignore wifi devices
	  -C, --get_battery_level      get battery current capacity
	  -O, --output <file>          write stdout to this file
	  -E, --error_output <file>    write stderr to this file
	  --detect_deadlocks <sec>     start printing backtraces for all threads periodically after specific amount of seconds
	  -f, --file_system            specify file system for mkdir / list / upload / download / rm
	  -F, --non-recursively        specify non-recursively walk directory
	  -S, --symbols                download OS symbols. must specify a directory to store the downloaded symbols
	  -j, --json                   format output as JSON
	  -k, --key                    keys for the properties of the bundle. Joined by ',' and used only with -B <list_bundle_id> and -j <json>
	  --custom-script <script>     path to custom python script to execute in lldb
	  --custom-command <command>   specify additional lldb commands to execute
	  --faster-path-search         use alternative logic to find the device support paths faster
	  -P, --list_profiles          list all provisioning profiles on device
	  --profile-uuid <uuid>        the UUID of the provisioning profile to target, use with other profile commands
	  --profile-download <path>    download a provisioning profile (requires --profile-uuid)
	  --profile-install <file>     install a provisioning profile
	  --profile-uninstall          uninstall a provisioning profile (requires --profile-uuid <UUID>)
	  --check-developer-mode       checks whether the given device has developer mode enabled (Requires Xcode 14 or newer)

## Examples

The commands below assume that you have an app called `my.app` with bundle id `bundle.id`. Substitute where necessary.

    // deploy and debug your app to a connected device
    ios-deploy --debug --bundle my.app

    // deploy, debug and pass environment variables to a connected device
    ios-deploy --debug --envs DYLD_PRINT_STATISTICS=1 --bundle my.app

    // deploy and debug your app to a connected device, skipping any wi-fi connection (use USB)
    ios-deploy --debug --bundle my.app --no-wifi

    // deploy and launch your app to a connected device, but quit the debugger after
    ios-deploy --justlaunch --debug --bundle my.app

    // deploy and launch your app to a connected device, quit when app crashes or exits
    ios-deploy --noninteractive --debug --bundle my.app

    // deploy your app to a connected device using incremental installation
    ios-deploy --app_deltas /tmp --bundle my.app

    // Upload a file to your app's Documents folder
    ios-deploy --bundle_id 'bundle.id' --upload test.txt --to Documents/test.txt

    // Download your app's Documents, Library and tmp folders
    ios-deploy --bundle_id 'bundle.id' --download --to MyDestinationFolder

    // List the contents of your app's Documents, Library and tmp folders
    ios-deploy --bundle_id 'bundle.id' --list

    // deploy and debug your app to a connected device, uninstall the app first
    ios-deploy --uninstall --debug --bundle my.app

    // check whether an app by bundle id exists on the device (check return code `echo $?`)
    ios-deploy --exists --bundle_id com.apple.mobilemail

    // Download the Documents directory of the app *only*
    ios-deploy --download=/Documents --bundle_id my.app.id --to ./my_download_location
    
    // List ids and names of connected devices
    ios-deploy -c
    
    // Uninstall an app
    ios-deploy --uninstall_only --bundle_id my.bundle.id
    
    // list all bundle ids of all apps on your device
    ios-deploy --list_bundle_id
    
    // list the files in cameral roll, a.k.a /DCIM
    ios-deploy -f -l/DCIM
    
    // download the file in /DCIM
    ios-deploy -f -w/DCIM/100APPLE/IMG_001.jpg
    
    // remove the file /DCIM
    ios-deploy -f -R /DCIM/100APPLE/IMG_001.jpg
    
    // make directoly in /DCIM
    ios-deploy -f -D/DCIM/test
    
    // upload file to /DCIM
    ios-deploy -f -o/Users/ryan/Downloads/test.png -2/DCIM/test.png
    
    // get more properties of the bundle
    ios-deploy -B -j --key=UIFileSharingEnabled,CFBundlePackageType
    ios-deploy -B -j --key=UIFileSharingEnabled --key=CFBundlePackageType


## Demo

The included demo.app represents the minimum required to get code running on iOS.

* `make demo.app` will generate the demo.app executable. If it doesn't compile, modify `IOS_SDK_VERSION` in the Makefile.
* `make debug` will install demo.app and launch a LLDB session.

## Notes

* `--detect_deadlocks` can help to identify an exact state of application's threads in case of a deadlock. It works like this: The user specifies the amount of time ios-deploy runs the app as usual. When the timeout is elapsed ios-deploy starts to print call-stacks of all threads every 5 seconds and the app keeps running. Comparing threads' call-stacks between each other helps to identify the threads which were stuck.

## License

ios-deploy is available under the provisions of the GNU General Public License,
version 3 (or later), available here: http://www.gnu.org/licenses/gpl-3.0.html


Error codes used for error messages were taken from SDMMobileDevice framework,
originally reverse engineered by Sam Marshall. SDMMobileDevice is distributed
under BSD 3-Clause license and is available here:
https://github.com/samdmarshall/SDMMobileDevice
