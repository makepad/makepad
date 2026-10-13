package dev.makepad.run;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/** The HAL owns MakepadNative from library load; source compilation never gates the Activity. */
public class RunActivity extends Activity {
    static {
        System.loadLibrary("makepad");
    }

    static native void boot(String apkPath, String filesDir, String cacheDir, String libDir,
        android.content.res.AssetManager assets, String app, String remote);
    public static native String status();

    /** The Activity that booted this process: what a restart starts again. */
    static Activity current;

    /** A new process boots the app again (the window manager's settings, "restart now"): the
     *  Activity's task is started anew and this process is killed (SIGKILL: no exit-time work
     *  runs under the program's threads). Any thread. */
    public static void restartProcess() {
        final Activity a = current;
        if (a == null) return;
        a.runOnUiThread(new Runnable() { public void run() {
            Intent i = Intent.makeRestartActivityTask(new android.content.ComponentName(a, RunActivity.class));
            a.startActivity(i);
            android.os.Process.killProcess(android.os.Process.myPid());
        }});
    }

    /** Also called when Android restores MakepadApp directly in a new process. */
    public static void ensureBoot(Activity activity) {
        current = activity;
        boot(activity.getPackageCodePath(), activity.getFilesDir().getAbsolutePath(),
            activity.getCacheDir().getAbsolutePath(), activity.getApplicationInfo().nativeLibraryDir,
            activity.getAssets(), activity.getIntent().hasExtra("app")
                ? activity.getIntent().getStringExtra("app") : "",
            // `am start --es remote "<hub>,<key hex>,<fingerprint>,<name>"`: the host dials this hub (makepad-remote)
            activity.getIntent().hasExtra("remote") ? activity.getIntent().getStringExtra("remote") : "");
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // MakepadApp starts compilation after its first HAL callbacks have captured the Activity.
        Intent intent = new Intent(this, MakepadApp.class);
        if (getIntent().hasExtra("app")) {
            intent.putExtra("app", getIntent().getStringExtra("app"));
        }
        if (getIntent().hasExtra("remote")) {
            intent.putExtra("remote", getIntent().getStringExtra("remote"));
        }
        startActivity(intent);
        overridePendingTransition(0, 0);
        finish();
    }
}
