package nl.makepad.android;
import android.Manifest;

import android.os.Bundle;
import android.app.Activity;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.RelativeLayout;

public class MakepadActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        mCx = Makepad.newCx();
        setContentView(new MakepadSurfaceView(this, mCx));
        requestPermissions(new String[]{Manifest.permission.BLUETOOTH_CONNECT}, 123);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        Makepad.dropCx(mCx);
    }

    long mCx;
}