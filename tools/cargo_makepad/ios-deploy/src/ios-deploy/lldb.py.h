"# AUTO-GENERATED - DO NOT MODIFY\n"
"import time\n"
"import os\n"
"import sys\n"
"import shlex\n"
"import lldb\n"
"\n"
"listener = None\n"
"startup_error = lldb.SBError()\n"
"\n"
"def connect_command(debugger, command, result, internal_dict):\n"
"    # These two are passed in by the script which loads us\n"
"    connect_url = internal_dict['fruitstrap_connect_url']\n"
"    error = lldb.SBError()\n"
"    \n"
"    # We create a new listener here and will use it for both target and the process.\n"
"    # It allows us to prevent data races when both our code and internal lldb code\n"
"    # try to process STDOUT/STDERR messages\n"
"    global listener\n"
"    listener = lldb.SBListener('iosdeploy_listener')\n"
"    \n"
"    listener.StartListeningForEventClass(debugger,\n"
"                                            lldb.SBProcess.GetBroadcasterClassName(),\n"
"                                            lldb.SBProcess.eBroadcastBitStateChanged | lldb.SBProcess.eBroadcastBitSTDOUT | lldb.SBProcess.eBroadcastBitSTDERR)\n"
"    \n"
"    process = debugger.GetSelectedTarget().ConnectRemote(listener, connect_url, None, error)\n"
"\n"
"    # Wait for connection to succeed\n"
"    events = []\n"
"    state = (process.GetState() or lldb.eStateInvalid)\n"
"\n"
"    while state != lldb.eStateConnected:\n"
"        event = lldb.SBEvent()\n"
"        if listener.WaitForEvent(1, event):\n"
"            state = process.GetStateFromEvent(event)\n"
"            events.append(event)\n"
"        else:\n"
"            state = lldb.eStateInvalid\n"
"\n"
"    # Add events back to queue, otherwise lldb freezes\n"
"    for event in events:\n"
"        listener.AddEvent(event)\n"
"\n"
"def run_command(debugger, command, result, internal_dict):\n"
"    device_app = internal_dict['fruitstrap_device_app']\n"
"    args = command.split('--',1)\n"
"    debugger.GetSelectedTarget().modules[0].SetPlatformFileSpec(lldb.SBFileSpec(device_app))\n"
"    args_arr = []\n"
"    if len(args) > 1:\n"
"        args_arr = shlex.split(args[1])\n"
"    args_arr = args_arr + shlex.split('{args}')\n"
"\n"
"    launchInfo = lldb.SBLaunchInfo(args_arr)\n"
"    global listener\n"
"    launchInfo.SetListener(listener)\n"
"    \n"
"    #This env variable makes NSLog, CFLog and os_log messages get mirrored to stderr\n"
"    #https://stackoverflow.com/a/39581193 \n"
"    launchInfo.SetEnvironmentEntries(['OS_ACTIVITY_DT_MODE=enable'], True)\n"
"\n"
"    envs_arr = []\n"
"    if len(args) > 1:\n"
"        envs_arr = shlex.split(args[1])\n"
"    envs_arr = envs_arr + shlex.split('{envs}')\n"
"    launchInfo.SetEnvironmentEntries(envs_arr, True)\n"
"    \n"
"    debugger.GetSelectedTarget().Launch(launchInfo, startup_error)\n"
"    lockedstr = ': Locked'\n"
"    if lockedstr in str(startup_error):\n"
"       print('\\nDevice Locked\\n')\n"
"       os._exit(254)\n"
"    else:\n"
"       print(str(startup_error))\n"
"\n"
"def safequit_command(debugger, command, result, internal_dict):\n"
"    process = debugger.GetSelectedTarget().process\n"
"    state = process.GetState()\n"
"    if state == lldb.eStateRunning:\n"
"        process.Detach()\n"
"        os._exit(0)\n"
"    elif state > lldb.eStateRunning:\n"
"        os._exit(state)\n"
"    else:\n"
"        print('\\nApplication has not been launched\\n')\n"
"        os._exit(1)\n"
"\n"
"\n"
"def print_stacktrace(thread):\n"
"    # Somewhere between Xcode-13.2.1 and Xcode-13.3 lldb starts to throw an error during printing of backtrace.\n"
"    # Manually write the backtrace out so we don't just get 'invalid thread'.\n"
"    sys.stdout.write('  ' + str(thread) + '\\n')\n"
"    for frame in thread:\n"
"        out = lldb.SBStream()\n"
"        frame.GetDescription(out)\n"
"        sys.stdout.write(' ' * 4 + out.GetData())\n"
"\n"
"def autoexit_command(debugger, command, result, internal_dict):\n"
"    global listener\n"
"    process = debugger.GetSelectedTarget().process\n"
"    if not startup_error.Success():\n"
"        print('\\nPROCESS_NOT_STARTED\\n')\n"
"        os._exit({exitcode_app_crash})\n"
"\n"
"    output_path = internal_dict['fruitstrap_output_path']\n"
"    out = None\n"
"    if output_path:\n"
"        out = open(output_path, 'w')\n"
"\n"
"    error_path = internal_dict['fruitstrap_error_path']\n"
"    err = None\n"
"    if error_path:\n"
"        err = open(error_path, 'w')\n"
"\n"
"    detectDeadlockTimeout = {detect_deadlock_timeout}\n"
"    printBacktraceTime = time.time() + detectDeadlockTimeout if detectDeadlockTimeout > 0 else None\n"
"    \n"
"    # This line prevents internal lldb listener from processing STDOUT/STDERR/StateChanged messages.\n"
"    # Without it, an order of log writes is incorrect sometimes\n"
"    debugger.GetListener().StopListeningForEvents(process.GetBroadcaster(),\n"
"                                                  lldb.SBProcess.eBroadcastBitSTDOUT | lldb.SBProcess.eBroadcastBitSTDERR | lldb.SBProcess.eBroadcastBitStateChanged )\n"
"\n"
"    event = lldb.SBEvent()\n"
"    \n"
"    def ProcessSTDOUT():\n"
"        stdout = process.GetSTDOUT(1024)\n"
"        while stdout:\n"
"            if out:\n"
"                out.write(stdout)\n"
"            else:\n"
"                sys.stdout.write(stdout)\n"
"            stdout = process.GetSTDOUT(1024)\n"
"\n"
"    def ProcessSTDERR():\n"
"        stderr = process.GetSTDERR(1024)\n"
"        while stderr:\n"
"            if err:\n"
"                err.write(stderr)\n"
"            else:\n"
"                sys.stdout.write(stderr)\n"
"            stderr = process.GetSTDERR(1024)\n"
"\n"
"    def CloseOut():\n"
"        sys.stdout.flush()\n"
"        if (out):\n"
"            out.close()\n"
"        if (err):\n"
"            err.close()\n"
"\n"
"    while True:\n"
"        if listener.WaitForEvent(1, event) and lldb.SBProcess.EventIsProcessEvent(event):\n"
"            state = lldb.SBProcess.GetStateFromEvent(event)\n"
"            type = event.GetType()\n"
"        \n"
"            if type & lldb.SBProcess.eBroadcastBitSTDOUT:\n"
"                ProcessSTDOUT()\n"
"        \n"
"            if type & lldb.SBProcess.eBroadcastBitSTDERR:\n"
"                ProcessSTDERR()\n"
"    \n"
"        else:\n"
"            state = process.GetState()\n"
"\n"
"        if state != lldb.eStateRunning:\n"
"            # Let's make sure that we drained our streams before exit\n"
"            ProcessSTDOUT()\n"
"            ProcessSTDERR()\n"
"\n"
"        if state == lldb.eStateExited:\n"
"            sys.stdout.write( '\\nPROCESS_EXITED\\n' )\n"
"            CloseOut()\n"
"            os._exit(process.GetExitStatus())\n"
"        elif printBacktraceTime is None and state == lldb.eStateStopped:\n"
"            selectedThread = process.GetSelectedThread()\n"
"            if selectedThread.GetStopReason() == lldb.eStopReasonNone:\n"
"                # During startup there are some stops for lldb to setup properly.\n"
"                # On iOS-16 we receive them with stop reason none.\n"
"                continue\n"
"            sys.stdout.write( '\\nPROCESS_STOPPED\\n' )\n"
"            print_stacktrace(process.GetSelectedThread())\n"
"            CloseOut()\n"
"            os._exit({exitcode_app_crash})\n"
"        elif state == lldb.eStateCrashed:\n"
"            sys.stdout.write( '\\nPROCESS_CRASHED\\n' )\n"
"            print_stacktrace(process.GetSelectedThread())\n"
"            CloseOut()\n"
"            os._exit({exitcode_app_crash})\n"
"        elif state == lldb.eStateDetached:\n"
"            sys.stdout.write( '\\nPROCESS_DETACHED\\n' )\n"
"            CloseOut()\n"
"            os._exit({exitcode_app_crash})\n"
"        elif printBacktraceTime is not None and time.time() >= printBacktraceTime:\n"
"            printBacktraceTime = None\n"
"            sys.stdout.write( '\\nPRINT_BACKTRACE_TIMEOUT\\n' )\n"
"            debugger.HandleCommand('process interrupt')\n"
"            for thread in process:\n"
"                print_stacktrace(thread)\n"
"                sys.stdout.write('\\n')\n"
"            debugger.HandleCommand('continue')\n"
"            printBacktraceTime = time.time() + 5\n"
