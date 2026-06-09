package dev.makepad.android;

import android.app.Activity;
import android.app.ActivityManager;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ComponentCallbacks2;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.Manifest;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Insets;
import android.graphics.Rect;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.hardware.input.InputManager;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.media.MediaFormat;
import android.media.midi.MidiDevice;
import android.media.midi.MidiDeviceInfo;
import android.media.midi.MidiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Log;
import android.view.ActionMode;
import android.view.Display;
import android.view.InputDevice;
import android.view.Menu;
import android.view.MenuItem;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.PixelCopy;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.view.WindowManager.LayoutParams;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.ExtractedText;
import android.view.inputmethod.ExtractedTextRequest;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.SpannableStringBuilder;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Set;
import java.util.concurrent.CompletableFuture;

// note: //% is a special miniquad's pre-processor for plugins
// when there are no plugins - //% whatever will be replaced to an empty string
// before compiling

//% IMPORTS

class MakepadImeInsets {
    private static final int MIN_KEYBOARD_HEIGHT_DP = 80;

    // True while a soft-keyboard (IME) show/hide animation is running. While an
    // animation is in flight the per-frame WindowInsetsAnimation callback
    // (onProgress/onEnd) is the authoritative inset source; the layout-driven
    // fallbacks (onApplyWindowInsets, onGlobalLayout) observe a contradictory
    // mix of target and stale insets mid-animation, so they defer to it.
    static boolean imeAnimationInProgress = false;

    private static int keyboardThresholdPx(View view) {
        return Math.max(1, (int) (view.getResources().getDisplayMetrics().density * MIN_KEYBOARD_HEIGHT_DP));
    }

    private static int rootHeightPx(View view) {
        View root = view.getRootView();
        return root == null ? 0 : root.getHeight();
    }

    // Distance in pixels from the bottom edge of the render surface up to the
    // bottom edge of the window. Zero when the window is edge-to-edge; equal to
    // the navigation-bar height when it is not (the framework then lays the
    // surface out above the navigation bar). The IME and visible-frame
    // measurements below are relative to the window bottom, so this gap must be
    // subtracted to get the IME's overlap with the surface itself.
    private static int surfaceBottomGapPx(View view) {
        View root = view.getRootView();
        if (root == null || root.getHeight() <= 0 || view.getHeight() <= 0) {
            return 0;
        }
        int[] loc = new int[2];
        view.getLocationInWindow(loc);
        int surfaceBottom = loc[1] + view.getHeight();
        return Math.max(0, root.getHeight() - surfaceBottom);
    }

    private static int clampToRootHeight(View view, int overlap) {
        int rootHeight = rootHeightPx(view);
        if (rootHeight <= 0 || overlap <= 0) {
            return 0;
        }
        return Math.min(overlap, rootHeight);
    }

    private static boolean isNearFullHeightOverlap(View view, int overlap) {
        int rootHeight = rootHeightPx(view);
        return rootHeight > 0 && rootHeight - overlap <= keyboardThresholdPx(view);
    }

    private static int visibleFrameBottomOverlapPx(View view) {
        Rect visibleFrame = new Rect();
        view.getWindowVisibleDisplayFrame(visibleFrame);

        View root = view.getRootView();
        if (root == null || root.getHeight() <= 0) {
            return 0;
        }

        int[] rootLocation = new int[2];
        root.getLocationOnScreen(rootLocation);
        if (visibleFrame.isEmpty() || visibleFrame.bottom <= rootLocation[1]) {
            return 0;
        }

        int rootBottomOnScreen = rootLocation[1] + root.getHeight();
        // visibleFrame.bottom is relative to the window; subtract the gap below
        // the surface so the fallback also measures overlap with the surface.
        return Math.max(0, rootBottomOnScreen - visibleFrame.bottom - surfaceBottomGapPx(view));
    }

    static int bottomOverlapPx(View view, WindowInsets insets) {
        int imeBottom = 0;
        if (Build.VERSION.SDK_INT >= 30 && insets != null) {
            Insets imeInsets = insets.getInsets(WindowInsets.Type.ime());
            // imeInsets.bottom is measured from the window bottom; subtract the
            // gap below the surface so only the IME's overlap with the surface
            // shifts content (a non-edge-to-edge window would otherwise
            // over-shift the content by the navigation-bar height).
            int imeOverlap = Math.max(0, imeInsets.bottom - surfaceBottomGapPx(view));
            imeBottom = clampToRootHeight(view, imeOverlap);
        }

        if (imeBottom > 0 && !isNearFullHeightOverlap(view, imeBottom)) {
            return imeBottom;
        }

        // Fallback for Android/OEM paths where Type.ime().bottom reports 0,
        // most commonly landscape keyboards. Using only the bottom edge avoids
        // counting status-bar differences at the top of the window.
        int fallback = visibleFrameBottomOverlapPx(view);
        if (fallback <= keyboardThresholdPx(view)) {
            return 0;
        }

        // A visible frame that is basically empty is not a keyboard measurement;
        // it is a transient/invalid layout result. Do not turn it into a
        // near-full-screen IME height.
        if (isNearFullHeightOverlap(view, fallback)) {
            return 0;
        }
        return clampToRootHeight(view, fallback);
    }

    // Whether the IME should be reported as "open" to native code.
    //
    // This must reflect the *target* (settled) IME visibility, not the
    // per-frame animated inset. During a show animation onProgress() delivers
    // insets whose IME height ramps up from 0, and at height 0 those animated
    // insets report isVisible(ime)==false — treating that first frame as
    // "closed" makes showing the keyboard look like an instant dismissal (and
    // the native side then actually hides it). getRootWindowInsets() reflects
    // the requested IME visibility and stays stable for the whole animation,
    // so it is the authoritative source for the open/closed flag; the
    // per-frame bottomOverlap height still drives the content-shift animation.
    static boolean isVisible(View view, int bottomOverlapPx) {
        if (Build.VERSION.SDK_INT >= 30 && view != null) {
            WindowInsets root = view.getRootWindowInsets();
            if (root != null && root.isVisible(WindowInsets.Type.ime())) {
                // Target is "shown": open for the whole show animation, even
                // while the animated height is still ramping up from 0.
                return true;
            }
        }
        // Target is "hidden" (or pre-API-30): still open while the IME
        // occupies space, so a hide animation reports open until it finishes
        // collapsing and then closed.
        return bottomOverlapPx > 0;
    }

    static void report(View view, WindowInsets insets, String src) {
        // While an IME animation is running, only the per-frame
        // WindowInsetsAnimation callback (onProgress/onEnd) is authoritative.
        // The layout-driven fallbacks (onApplyWindowInsets, onGlobalLayout) see
        // contradictory insets mid-animation and would fight the animation
        // callback, so they defer to it.
        if (imeAnimationInProgress
                && (src.equals("onApplyWindowInsets") || src.equals("onGlobalLayout"))) {
            return;
        }
        int bottomOverlap = bottomOverlapPx(view, insets);
        boolean visible = isVisible(view, bottomOverlap);
        MakepadNative.surfaceOnResizeTextIME(bottomOverlap, visible);
    }
}

class MakepadSystemInsets {
    final float top;
    final float right;
    final float bottom;
    final float left;

    private MakepadSystemInsets(float top, float right, float bottom, float left) {
        this.top = top;
        this.right = right;
        this.bottom = bottom;
        this.left = left;
    }

    // Computes the safe-area insets the render surface actually needs.
    //
    // The system-bar + display-cutout insets describe bands at the *window*
    // edges. When the window is not edge-to-edge (the default below Android 15
    // / API 35, where targetSdk-35 edge-to-edge enforcement does not apply),
    // the framework already lays our content out *inside* the system bars, so
    // the surface does not overlap them at all. Reporting the raw window-edge
    // insets there would pad the content twice — once by the OS, once by
    // Makepad — leaving an oversized gap. To stay correct in both regimes we
    // report only the part of each bar band that actually overlaps the
    // surface's on-screen rectangle (the same overlap approach used for the
    // IME inset). Edge-to-edge: overlap == full bar size. Content inside the
    // bars: overlap == 0.
    @SuppressWarnings("deprecation")
    static MakepadSystemInsets from(View view, WindowInsets insets, float density) {
        if (insets == null || view == null || density <= 0.0f) {
            return new MakepadSystemInsets(0, 0, 0, 0);
        }

        // Raw system-bar + display-cutout insets, in pixels, at the window edges.
        int barTop, barRight, barBottom, barLeft;
        if (Build.VERSION.SDK_INT >= 30) {
            Insets bars = insets.getInsets(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout()
            );
            barTop = bars.top;
            barRight = bars.right;
            barBottom = bars.bottom;
            barLeft = bars.left;
        } else {
            barTop = insets.getSystemWindowInsetTop();
            barRight = insets.getSystemWindowInsetRight();
            barBottom = insets.getSystemWindowInsetBottom();
            barLeft = insets.getSystemWindowInsetLeft();
        }

        View root = view.getRootView();
        if (root == null || root.getWidth() <= 0 || root.getHeight() <= 0) {
            return new MakepadSystemInsets(0, 0, 0, 0);
        }
        int windowWidth = root.getWidth();
        int windowHeight = root.getHeight();

        // The surface rectangle in window coordinates (same space as the
        // system-bar insets above). An un-laid-out view yields a degenerate
        // rect, which the intersections below collapse to zero insets.
        int[] loc = new int[2];
        view.getLocationInWindow(loc);
        int surfaceLeft = loc[0];
        int surfaceTop = loc[1];
        int surfaceRight = surfaceLeft + view.getWidth();
        int surfaceBottom = surfaceTop + view.getHeight();

        // Intersection length of each window-edge bar band with the surface.
        int top = Math.max(0,
            Math.min(barTop, surfaceBottom) - Math.max(0, surfaceTop));
        int left = Math.max(0,
            Math.min(barLeft, surfaceRight) - Math.max(0, surfaceLeft));
        int bottom = Math.max(0,
            Math.min(windowHeight, surfaceBottom) - Math.max(windowHeight - barBottom, surfaceTop));
        int right = Math.max(0,
            Math.min(windowWidth, surfaceRight) - Math.max(windowWidth - barRight, surfaceLeft));

        return new MakepadSystemInsets(
            top / density,
            right / density,
            bottom / density,
            left / density
        );
    }

    // Computes the safe-area (system-bar + display-cutout) insets and pushes
    // them to native code. Called from both ResizingLayout.onApplyWindowInsets
    // (the primary inset dispatch) and MakepadSurface.onGlobalLayout (a
    // per-layout fallback). The fallback is what makes the safe area correct
    // from launch: onApplyWindowInsets is not reliably dispatched with settled
    // system-bar insets on a cold start, so without the fallback the app
    // renders edge-to-edge (content under the status bar) until an IME show or
    // a rotation forces a fresh inset dispatch. The native side dedups
    // unchanged values, so calling this on every layout pass is cheap.
    static void report(View view, WindowInsets insets) {
        float density = view.getResources().getDisplayMetrics().density;
        MakepadSystemInsets i = MakepadSystemInsets.from(view, insets, density);
        MakepadNative.surfaceOnSafeAreaInsets(i.top, i.right, i.bottom, i.left);
    }
}

class MakepadSurface
    extends
        SurfaceView
    implements
        View.OnTouchListener,
        View.OnKeyListener,
        View.OnLongClickListener,
        ViewTreeObserver.OnGlobalLayoutListener,
        SurfaceHolder.Callback
{
    // IME InputConnection for handling composition text
    private MakepadInputConnection mInputConnection;

    // Shared Editable buffer for IME - this is the source of truth for Java side
    private SpannableStringBuilder mEditable = new SpannableStringBuilder();

    // Keyboard configuration constants (must match Rust KeyboardType enum)
    static final int INPUT_MODE_TEXT = 0;
    static final int INPUT_MODE_ASCII = 1;
    static final int INPUT_MODE_URL = 2;
    static final int INPUT_MODE_NUMERIC = 3;
    static final int INPUT_MODE_TEL = 4;
    static final int INPUT_MODE_EMAIL = 5;
    static final int INPUT_MODE_DECIMAL = 6;
    static final int INPUT_MODE_SEARCH = 7;
    static final int INPUT_MODE_NONE = 8;

    // Autocapitalize constants (must match Rust Autocapitalize enum)
    static final int AUTOCAP_NONE = 0;
    static final int AUTOCAP_WORDS = 1;
    static final int AUTOCAP_SENTENCES = 2;
    static final int AUTOCAP_ALL = 3;

    // Autocorrect constants (must match Rust Autocorrect enum)
    static final int AUTOCORRECT_DEFAULT = 0;
    static final int AUTOCORRECT_YES = 1;
    static final int AUTOCORRECT_NO = 2;

    // Return key type constants (must match Rust ReturnKeyType enum)
    static final int RETURN_KEY_DEFAULT = 0;
    static final int RETURN_KEY_GO = 1;
    static final int RETURN_KEY_SEARCH = 2;
    static final int RETURN_KEY_SEND = 3;
    static final int RETURN_KEY_NEXT = 4;
    static final int RETURN_KEY_DONE = 5;
    static final int RETURN_KEY_NONE = 6;
    static final int RETURN_KEY_PREVIOUS = 7;

    // Keyboard configuration (set by Rust via configureKeyboard)
    private int mInputMode = INPUT_MODE_TEXT;
    private int mAutocapitalize = AUTOCAP_SENTENCES;
    private int mAutocorrect = AUTOCORRECT_DEFAULT;
    private int mReturnKeyType = RETURN_KEY_DEFAULT;
    private boolean mIsMultiline = true;
    private boolean mIsSecure = false;

    // Package-private getters for MakepadInputConnection to access shared state
    Editable getEditable() {
        return mEditable;
    }

    int getInputMode() {
        return mInputMode;
    }

    boolean isMultiline() {
        return mIsMultiline;
    }

    // The X,Y coordinates and pointer ID of the most recent ACTION_DOWN touch.
    private float latestDownTouchX = Float.NaN;
    private float latestDownTouchY = Float.NaN;
    private int latestDownTouchPointerId = -1;

    // The X,Y coordinates and pointer ID of the most recent non-ACTION_DOWN touch event.
    private float latestTouchX = Float.NaN;
    private float latestTouchY = Float.NaN;
    private int latestTouchPointerId = -1;


    public MakepadSurface(Context context){
        super(context);
        getHolder().addCallback(this);

        setFocusable(true);
        setFocusableInTouchMode(true);
        requestFocus();
        setOnTouchListener(this);
        setOnKeyListener(this);
        setOnLongClickListener(this);

        getViewTreeObserver().addOnGlobalLayoutListener(this);

        Selection.setSelection(mEditable, 0, 0);
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Surface surface = holder.getSurface();
        //surface.setFrameRate(120f,0);
        MakepadNative.surfaceOnSurfaceCreated(surface);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        Context context = getContext();
        if (context instanceof MakepadActivity) {
            MakepadActivity activity = (MakepadActivity) context;
            if (activity.hasRecoverySnapshotAvailable()) {
                activity.setSurfaceCoverVisible(true);
            }
        }
        Surface surface = holder.getSurface();
        MakepadNative.surfaceOnSurfaceDestroyed(surface);
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder,
                               int format,
                               int width,
                               int height) {
        Surface surface = holder.getSurface();
        //surface.setFrameRate(120f,0);
        MakepadNative.surfaceOnSurfaceChanged(surface, width, height);

    }

    @Override
    public boolean onTouch(View view, MotionEvent event) {
        // By default, we return false so that `onLongClick` will trigger.
        boolean retval = false;

        int actionMasked = event.getActionMasked();
        int index = event.getActionIndex();
        int pointerId = event.getPointerId(index);

        // Save the details of the latest touch-down event,
        // such that we can use them in the `onLongClick` method.
        if (actionMasked == MotionEvent.ACTION_DOWN) {
            latestDownTouchX = event.getX(index);
            latestDownTouchY = event.getY(index);
            latestDownTouchPointerId = pointerId;
            // Re-set the latestTouchX/Y values on each down-touch.
            latestTouchX = latestDownTouchX;
            latestTouchY = latestDownTouchY;
            latestTouchPointerId = -1;
        }
        else if (actionMasked == MotionEvent.ACTION_MOVE) {
            latestTouchX = event.getX(index);
            latestTouchY = event.getY(index);
            latestTouchPointerId = pointerId;
            if (pointerId == latestDownTouchPointerId) {
                if (isTouchBeyondSlopDistance(view)) {
                    retval = true;
                }
            }
        }

        MakepadNative.surfaceOnTouch(event);
        return retval;
    }

    @Override
    public boolean onLongClick(View view) {
        long timeMillis = SystemClock.uptimeMillis();

        if (isTouchBeyondSlopDistance(view)) {
            return false;
        }

        // Here: a valid long click did occur, and we should send that event to makepad.

        // Use the latest touch coordinates if they're the same pointer ID as the initial down touch.
        if (latestTouchPointerId == latestDownTouchPointerId) {
            MakepadNative.surfaceOnLongClick(latestTouchX, latestTouchY, latestDownTouchPointerId, timeMillis);
        }
        // Otherwise, use the coordinates from the original down touch.
        else {
            MakepadNative.surfaceOnLongClick(latestDownTouchX, latestDownTouchY, latestDownTouchPointerId, timeMillis);
        }

        // Returning true here indicates that we have handled the long click event,
        // which triggers the haptic feedback (vibration motor) to buzz.
        return true;
    }

    // Returns true if the distance from the latest touch event to the prior down-touch event
    // is greated than the touch slop distance.
    //
    // If true, this indicates that the touch event shouldn't be considered a press/tap,
    // and is likely a drag or swipe.
    private boolean isTouchBeyondSlopDistance(View view) {
        int touchSlop = ViewConfiguration.get(view.getContext()).getScaledTouchSlop();
        float deltaX = latestTouchX - latestDownTouchX;
        float deltaY = latestTouchY - latestDownTouchY;
        double dist = Math.sqrt((deltaX * deltaX) + (deltaY * deltaY));
        return dist > touchSlop;
    }

    @Override
    public void onGlobalLayout() {
        // Fallback path: the parent ResizingLayout's OnApplyWindowInsetsListener
        // is the primary source of IME inset updates (it fires per-frame during
        // the keyboard animation on API 30+). This handler stays as a safety
        // net for layout changes that arrive without an inset dispatch, for
        // example, a focus change that retargets the IME to a different field.
        WindowInsets insets = this.getRootWindowInsets();
        MakepadImeInsets.report(this, insets, "onGlobalLayout");
        // Safe-area insets also flow through here. onApplyWindowInsets is not
        // reliably dispatched with settled system-bar insets on a cold start,
        // so without this the app renders edge-to-edge (content under the
        // status bar) until an IME show or rotation forces a fresh inset
        // dispatch. onGlobalLayout fires on every layout pass and picks up the
        // real insets as soon as the window settles.
        MakepadSystemInsets.report(this, insets);
    }

    // docs says getCharacters are deprecated
    // but somehow on non-latyn input all keyCode and all the relevant fields in the KeyEvent are zeros
    // and only getCharacters has some usefull data
    @SuppressWarnings("deprecation")
    @Override
    public boolean onKey(View v, int keyCode, KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN && keyCode != 0) {
            int metaState = event.getMetaState();
            boolean isRepeat = event.getRepeatCount() > 0;
            MakepadNative.surfaceOnKeyDown(keyCode, metaState, isRepeat);
        }

        if (event.getAction() == KeyEvent.ACTION_UP && keyCode != 0) {
            int metaState = event.getMetaState();
            MakepadNative.surfaceOnKeyUp(keyCode, metaState);
        }

        if (event.getAction() == KeyEvent.ACTION_UP || event.getAction() == KeyEvent.ACTION_MULTIPLE) {
            int character = event.getUnicodeChar();
            if (character == 0) {
                String characters = event.getCharacters();
                if (characters != null && characters.length() > 0) {
                    character = characters.charAt(0);
                }
            }

            if (character != 0) {
                MakepadNative.surfaceOnCharacter(character);
            }
        }

        if ((keyCode == KeyEvent.KEYCODE_VOLUME_UP) || (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN)) {
            return super.onKeyUp(keyCode, event);
        }

        return true;
    }

    // There is an Android bug when screen is in landscape,
    // the keyboard inset height is reported as 0.
    // This code is a workaround which fixes the bug.
    // See https://groups.google.com/g/android-developers/c/50XcWooqk7I
    // For some reason it only works if placed here and not in the parent layout.
    @Override
    public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
        int inputType = InputType.TYPE_CLASS_TEXT;

        switch (mInputMode) {
            case INPUT_MODE_NONE:
                inputType = InputType.TYPE_NULL;
                break;
            case INPUT_MODE_ASCII:
                // TYPE_TEXT_VARIATION_VISIBLE_PASSWORD shows ASCII keyboard without masking
                // This is the closest Android equivalent to iOS's UIKeyboardTypeASCIICapable
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD;
                break;
            case INPUT_MODE_URL:
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI;
                break;
            case INPUT_MODE_NUMERIC:
                inputType = InputType.TYPE_CLASS_NUMBER;
                break;
            case INPUT_MODE_TEL:
                inputType = InputType.TYPE_CLASS_PHONE;
                break;
            case INPUT_MODE_EMAIL:
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS;
                break;
            case INPUT_MODE_DECIMAL:
                inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL | InputType.TYPE_NUMBER_FLAG_SIGNED;
                break;
            case INPUT_MODE_SEARCH:
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_WEB_EDIT_TEXT;
                break;
            default: // INPUT_MODE_TEXT
                inputType = InputType.TYPE_CLASS_TEXT;
                break;
        }

        if ((inputType & InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_TEXT) {
            // Autocapitalization
            switch (mAutocapitalize) {
                case AUTOCAP_NONE:
                    // No flag needed
                    break;
                case AUTOCAP_WORDS:
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_WORDS;
                    break;
                case AUTOCAP_SENTENCES:
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_SENTENCES;
                    break;
                case AUTOCAP_ALL:
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS;
                    break;
            }

            // Autocorrect
            switch (mAutocorrect) {
                case AUTOCORRECT_DEFAULT:
                    break;
                case AUTOCORRECT_YES:
                    inputType |= InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
                    break;
                case AUTOCORRECT_NO:
                    inputType |= InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
                    break;
            }

            // Multiline - important for SwiftKey vertical cursor control
            if (mIsMultiline) {
                inputType |= InputType.TYPE_TEXT_FLAG_MULTI_LINE;
            }

            // Secure/password
            if (mIsSecure) {
                // Clear variation bits and set password variation
                inputType = (inputType & ~InputType.TYPE_MASK_VARIATION) | InputType.TYPE_TEXT_VARIATION_PASSWORD;
            }
        }

        outAttrs.inputType = inputType;

        int imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI;

        // Return key type
        switch (mReturnKeyType) {
            case RETURN_KEY_NONE:
                imeOptions |= EditorInfo.IME_ACTION_NONE;
                break;
            case RETURN_KEY_GO:
                imeOptions |= EditorInfo.IME_ACTION_GO;
                break;
            case RETURN_KEY_SEARCH:
                imeOptions |= EditorInfo.IME_ACTION_SEARCH;
                break;
            case RETURN_KEY_SEND:
                imeOptions |= EditorInfo.IME_ACTION_SEND;
                break;
            case RETURN_KEY_NEXT:
                imeOptions |= EditorInfo.IME_ACTION_NEXT;
                break;
            case RETURN_KEY_DONE:
                imeOptions |= EditorInfo.IME_ACTION_DONE;
                break;
            case RETURN_KEY_PREVIOUS:
                imeOptions |= EditorInfo.IME_ACTION_PREVIOUS;
                break;
            default: // RETURN_KEY_DEFAULT
                if (!mIsMultiline) {
                    imeOptions |= EditorInfo.IME_ACTION_DONE;
                } else {
                    imeOptions |= EditorInfo.IME_FLAG_NO_ENTER_ACTION;
                }
                break;
        }

        // Prevent personalized learning for secure/password fields
        if (mIsSecure) {
            imeOptions |= EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        }

        // Add IME_FLAG_FORCE_ASCII for ASCII input mode
        if (mInputMode == INPUT_MODE_ASCII) {
            imeOptions |= EditorInfo.IME_FLAG_FORCE_ASCII;
        }

        outAttrs.imeOptions = imeOptions;

        // Set initial selection from our Editable
        int selStart = Selection.getSelectionStart(mEditable);
        int selEnd = Selection.getSelectionEnd(mEditable);
        outAttrs.initialSelStart = Math.max(0, selStart);
        outAttrs.initialSelEnd = Math.max(0, selEnd);
        // EditorInfo.setInitialSurroundingSubText is API 30+. It's only an
        // optimization (it hands the IME the surrounding text up-front); on
        // older devices the IME just queries it on demand through the
        // InputConnection. Calling it unconditionally crashes API 26-29 with
        // NoSuchMethodError.
        if (Build.VERSION.SDK_INT >= 30) {
            outAttrs.setInitialSurroundingSubText(mEditable, 0);
        }

        // Create InputConnection with fullEditor=true since we have an Editable
        mInputConnection = new MakepadInputConnection(this, true);

        return mInputConnection;
    }

    // Configure keyboard settings - called from Rust before showing keyboard
    public void configureKeyboard(int inputMode, int autocapitalize, int autocorrect,
                                  int returnKeyType, boolean isMultiline, boolean isSecure) {
        boolean changed = (mInputMode != inputMode || mAutocapitalize != autocapitalize ||
                          mAutocorrect != autocorrect || mReturnKeyType != returnKeyType ||
                          mIsMultiline != isMultiline || mIsSecure != isSecure);

        mInputMode = inputMode;
        mAutocapitalize = autocapitalize;
        mAutocorrect = autocorrect;
        mReturnKeyType = returnKeyType;
        mIsMultiline = isMultiline;
        mIsSecure = isSecure;

        // If config changed and keyboard is already showing, restart input to apply new settings
        if (changed && mInputConnection != null) {
            // Finalize any in-progress composition before restart to avoid stale state
            BaseInputConnection.removeComposingSpans(mEditable);
            InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null) {
                imm.restartInput(this);
            }
        }
    }

    // Called from Rust to update text state (for programmatic changes, not IME input)
    public void updateImeTextState(String fullText, int selStart, int selEnd,
                                   int composingStart, int composingEnd) {
        String currentText = mEditable.toString();
        boolean textChanged = !currentText.equals(fullText);

        // ECHO PREVENTION: Check if this is Rust echoing back text we recently sent.
        // This happens because:
        //   1. Java sends text to Rust via onImeTextStateChanged
        //   2. Rust widget processes it and updates internal state
        //   3. Rust may sync state back via SyncImeState -> updateImeTextState
        //   4. Without this check, we'd overwrite fresh IME state with stale echo
        if (textChanged && mInputConnection != null) {
            if (mInputConnection.wasRecentlySentToRust(fullText)) {
                return;  // Stale echo - ignore to prevent rollback
            }
        }

        // Clamp selection
        int textLen = textChanged ? fullText.length() : currentText.length();
        selStart = Math.max(0, Math.min(selStart, textLen));
        selEnd = Math.max(selStart, Math.min(selEnd, textLen));
        boolean hasComposition = composingStart >= 0 && composingEnd >= composingStart;
        if (hasComposition) {
            composingStart = Math.max(0, Math.min(composingStart, textLen));
            composingEnd = Math.max(composingStart, Math.min(composingEnd, textLen));
        } else {
            composingStart = -1;
            composingEnd = -1;
        }

        if (textChanged) {
            // Text content changed - update Editable and notify IME
            BaseInputConnection.removeComposingSpans(mEditable);
            mEditable.replace(0, mEditable.length(), fullText);
            Selection.setSelection(mEditable, selStart, selEnd);
            if (hasComposition && mInputConnection != null) {
                mInputConnection.setComposingRegion(composingStart, composingEnd);
            }

            // ECHO PREVENTION: Clear the sent buffer after applying Rust's authoritative
            // state update. This ensures the next text we send to Rust won't be incorrectly
            // detected as an echo. Only clear here, NOT in recordSentToRust().
            if (mInputConnection != null) {
                mInputConnection.clearRecentSentBuffer();
            }

            // Notify IME of text change without restarting input
            // restartInput() destroys composition state and causes IME flicker;
            // updateExtractedText() + updateSelection() is the lightweight alternative
            if (mInputConnection != null) {
                InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) {
                    if (mInputConnection.mExtractedTextRequest != null) {
                        ExtractedText et = new ExtractedText();
                        et.text = fullText;
                        et.startOffset = 0;
                        et.selectionStart = selStart;
                        et.selectionEnd = selEnd;
                        imm.updateExtractedText(this, mInputConnection.mExtractedTextToken, et);
                    }
                    imm.updateSelection(this, selStart, selEnd, composingStart, composingEnd);
                }
            }
        } else {
            // Only selection changed - just update selection, no restart needed
            int currentSelStart = Selection.getSelectionStart(mEditable);
            int currentSelEnd = Selection.getSelectionEnd(mEditable);
            int currentCompStart = BaseInputConnection.getComposingSpanStart(mEditable);
            int currentCompEnd = BaseInputConnection.getComposingSpanEnd(mEditable);
            if (currentSelStart != selStart || currentSelEnd != selEnd
                    || currentCompStart != composingStart || currentCompEnd != composingEnd) {
                if (hasComposition && mInputConnection != null) {
                    mInputConnection.setComposingRegion(composingStart, composingEnd);
                } else {
                    BaseInputConnection.removeComposingSpans(mEditable);
                }
                Selection.setSelection(mEditable, selStart, selEnd);
                // Notify IME of selection change without restart
                InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) {
                    imm.updateSelection(this, selStart, selEnd, composingStart, composingEnd);
                }
            }
        }
    }

    public Surface getNativeSurface() {
        return getHolder().getSurface();
    }

    // Select all text in the InputConnection's Editable and notify IME
    // Used by ActionMode's Select All to sync Java-side selection with Rust
    public void selectAllInEditable() {
        int len = mEditable.length();
        Selection.setSelection(mEditable, 0, len);
        // Notify IME of the selection change
        if (mInputConnection != null) {
            mInputConnection.notifyImeOfSelectionUpdate();
        }
    }
}

class CameraPreviewSurface extends SurfaceView implements SurfaceHolder.Callback {
    private final long mVideoId;

    public CameraPreviewSurface(Context context, long videoId) {
        super(context);
        mVideoId = videoId;
        getHolder().addCallback(this);
        setZOrderMediaOverlay(true);
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Surface surface = holder.getSurface();
        if (surface != null) {
            MakepadNative.onCameraPreviewSurfaceReady(mVideoId, surface, getWidth(), getHeight());
        }
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        Surface surface = holder.getSurface();
        if (surface != null) {
            MakepadNative.onCameraPreviewSurfaceReady(mVideoId, surface, width, height);
        }
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        MakepadNative.onCameraPreviewSurfaceDestroyed(mVideoId);
    }
}

class SelectionHandleView extends View {
    public SelectionHandleView(Context context, int color, int sizePx) {
        super(context);
        GradientDrawable bg = new GradientDrawable();
        bg.setShape(GradientDrawable.OVAL);
        bg.setColor(color);
        setBackground(bg);
        setClickable(true);
        setFocusable(false);
        setLayoutParams(new FrameLayout.LayoutParams(sizePx, sizePx));
    }
}

class ResizingLayout
    extends
        LinearLayout
    implements
        View.OnApplyWindowInsetsListener {

    public ResizingLayout(Context context){
        super(context);
        // Keep a stable non-black fallback behind the SurfaceView for task snapshots
        // and system transition frames that cannot capture the separate surface layer.
        setBackgroundResource(R.drawable.makepad_launch_background);
        setOnApplyWindowInsetsListener(this);

        // The IME animation API (API 30+) gives us an authoritative per-frame
        // dispatch of the IME inset that does NOT depend on softInputMode or
        // on the listener returning the right thing. `onApplyWindowInsets`
        // alone is unreliable across Android versions and orientations
        // (we've observed it firing in landscape but not portrait, and on
        // some OEMs not at all). With this callback attached we are
        // guaranteed to hear about every IME show / hide / animation
        // progress event.
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            setWindowInsetsAnimationCallback(
                new android.view.WindowInsetsAnimation.Callback(
                    android.view.WindowInsetsAnimation.Callback.DISPATCH_MODE_CONTINUE_ON_SUBTREE
                ) {
                    @Override
                    public void onPrepare(android.view.WindowInsetsAnimation animation) {
                        if ((animation.getTypeMask() & WindowInsets.Type.ime()) != 0) {
                            MakepadImeInsets.imeAnimationInProgress = true;
                        }
                    }

                    @Override
                    public android.view.WindowInsets onProgress(
                        android.view.WindowInsets insets,
                        java.util.List<android.view.WindowInsetsAnimation> runningAnimations
                    ) {
                        MakepadImeInsets.report(ResizingLayout.this, insets, "onProgress");
                        return insets;
                    }

                    @Override
                    public void onEnd(android.view.WindowInsetsAnimation animation) {
                        if ((animation.getTypeMask() & WindowInsets.Type.ime()) != 0) {
                            MakepadImeInsets.imeAnimationInProgress = false;
                        }
                        // The framework usually delivers a final-state inset
                        // through onProgress just before onEnd, but on some
                        // OEM devices it skips that last frame. Fetch the
                        // current insets directly to make sure native code
                        // sees the settled state.
                        android.view.WindowInsets insets = getRootWindowInsets();
                        if (insets == null) return;
                        MakepadImeInsets.report(ResizingLayout.this, insets, "onEnd");
                    }
                }
            );
        }
    }

    @Override
    public WindowInsets onApplyWindowInsets(View v, WindowInsets insets) {
        // Report IME inset directly to native code. The in-app KeyboardView
        // is the single source of truth for shifting content above the soft
        // keyboard. We do not shrink the SurfaceView via setPadding; that
        // would double-count the obstruction (system shrinks the surface
        // *and* the KeyboardView shifts). The activity is configured with
        // `windowSoftInputMode="adjustNothing"` in the manifest, so the
        // system doesn't auto-resize either.
        MakepadImeInsets.report(v, insets, "onApplyWindowInsets");

        // Safe-area (system-bar + display-cutout) insets. Also reported from
        // MakepadSurface.onGlobalLayout as a cold-start fallback — see
        // MakepadSystemInsets.report.
        MakepadSystemInsets.report(v, insets);

        return insets;
    }
}

public class MakepadActivity
    extends Activity
    implements MidiManager.OnDeviceOpenedListener
{
    private static final String LOG_TAG = "Makepad";
    private static final long SURFACE_COVER_FADE_OUT_MS = 100;
    private static final long WARM_RESUME_SNAPSHOT_MAX_AGE_MS = 10000;
    private static final int TASK_DESCRIPTION_BACKGROUND_COLOR = 0xFFF5F7FA;
    private static Bitmap sWarmResumeSurfaceSnapshot;
    private static long sWarmResumeSurfaceSnapshotUptimeMs;
    private static int sWarmResumeSurfaceSnapshotOrientation = android.content.res.Configuration.ORIENTATION_UNDEFINED;
    //% MAIN_ACTIVITY_BODY

    private MakepadSurface view;
    private final Handler mHandler = new Handler(Looper.getMainLooper());
    private InputManager mInputManager;
    private InputManager.InputDeviceListener mInputDeviceListener;
    private Boolean mPhysicalKeyboardConnected;

    // video playback
    Handler mVideoPlaybackHandler;
    HandlerThread mVideoPlaybackThread;
    HashMap<Long, VideoPlayerRunnable> mVideoPlayerRunnables;

    // networking, make these static because of activity switching
    static HandlerThread mWebSocketsThread;
    static Handler mWebSocketsHandler;
    static HashMap<Long, MakepadWebSocket> mActiveWebsockets = new HashMap<>();
    static HashMap<Long, MakepadWebSocketReader> mActiveWebsocketsReaders = new HashMap<>();
    static HashMap<Long, MakepadSocketStream> mActiveSocketStreams = new HashMap<>();
    private boolean mIsSwitchingActivity = false;

    // Desired system-bar (status/navigation bar) icon tint, set from Rust via
    // setSystemBarAppearance(). true = dark icons (for light app backgrounds).
    private boolean mSystemBarDarkIcons = false;

    // clipboard actions (ActionMode for copy/paste/cut)
    private ActionMode mActionMode;
    private boolean mHasSelection = false;
    private int[] mSelectionBounds = new int[4]; // left, top, right, bottom
    private int mKeyboardShift = 0; // keyboard shift amount from Rust

    // native camera preview overlays
    private FrameLayout mRootLayout;
    private FrameLayout mSurfaceCoverOverlay;
    private ImageView mSurfaceSnapshotBackdrop;
    private ImageView mSurfaceSnapshotOverlay;
    private FrameLayout mCameraPreviewOverlay;
    private HashMap<Long, CameraPreviewSurface> mCameraPreviewViews = new HashMap<>();
    private Bitmap mLatestSurfaceSnapshot;
    private int mLatestSurfaceSnapshotOrientation = android.content.res.Configuration.ORIENTATION_UNDEFINED;
    private boolean mSurfaceSnapshotCopyInFlight = false;
    private boolean mSurfaceRecoveryOverlayVisible = false;

    // selection handles overlay
    private static final int SELECTION_HANDLE_START = 0;
    private static final int SELECTION_HANDLE_END = 1;
    private static final int SELECTION_DRAG_BEGIN = 0;
    private static final int SELECTION_DRAG_MOVE = 1;
    private static final int SELECTION_DRAG_END = 2;
    private FrameLayout mSelectionHandleOverlay;
    private SelectionHandleView mSelectionHandleStart;
    private SelectionHandleView mSelectionHandleEnd;
    private int mSelectionHandleSizePx;

    static {
        System.loadLibrary("makepad");
    }

    private boolean isPhysicalTextKeyboard(InputDevice device) {
        return device != null
            && device.isEnabled()
            && !device.isVirtual()
            && device.supportsSource(InputDevice.SOURCE_KEYBOARD)
            && device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC;
    }

    private boolean hasPhysicalTextKeyboard() {
        if (mInputManager == null) {
            return false;
        }
        for (int id : mInputManager.getInputDeviceIds()) {
            if (isPhysicalTextKeyboard(mInputManager.getInputDevice(id))) {
                return true;
            }
        }
        return false;
    }

    private void reportPhysicalKeyboardIfChanged() {
        boolean connected = hasPhysicalTextKeyboard();
        if (mPhysicalKeyboardConnected != null
                && mPhysicalKeyboardConnected.booleanValue() == connected) {
            return;
        }
        mPhysicalKeyboardConnected = Boolean.valueOf(connected);
        MakepadNative.surfaceOnPhysicalKeyboardChanged(connected);
    }

    private void registerPhysicalKeyboardListener() {
        if (mInputManager == null) {
            mInputManager = (InputManager) getSystemService(Context.INPUT_SERVICE);
        }
        if (mInputManager == null) {
            return;
        }
        if (mInputDeviceListener == null) {
            mInputDeviceListener = new InputManager.InputDeviceListener() {
                @Override
                public void onInputDeviceAdded(int deviceId) {
                    reportPhysicalKeyboardIfChanged();
                }

                @Override
                public void onInputDeviceRemoved(int deviceId) {
                    reportPhysicalKeyboardIfChanged();
                }

                @Override
                public void onInputDeviceChanged(int deviceId) {
                    reportPhysicalKeyboardIfChanged();
                }
            };
            mInputManager.registerInputDeviceListener(mInputDeviceListener, mHandler);
        }
        reportPhysicalKeyboardIfChanged();
    }

    private void unregisterPhysicalKeyboardListener() {
        if (mInputManager != null && mInputDeviceListener != null) {
            mInputManager.unregisterInputDeviceListener(mInputDeviceListener);
        }
        mInputDeviceListener = null;
        mInputManager = null;
    }

    private void cacheWarmResumeSurfaceSnapshot(Bitmap snapshot) {
        if (snapshot == null) {
            return;
        }
        sWarmResumeSurfaceSnapshot = snapshot;
        sWarmResumeSurfaceSnapshotUptimeMs = SystemClock.uptimeMillis();
        sWarmResumeSurfaceSnapshotOrientation = getResources().getConfiguration().orientation;
    }

    private boolean canRestoreWarmResumeSurfaceSnapshot() {
        if (sWarmResumeSurfaceSnapshot == null) {
            return false;
        }
        if (SystemClock.uptimeMillis() - sWarmResumeSurfaceSnapshotUptimeMs > WARM_RESUME_SNAPSHOT_MAX_AGE_MS) {
            clearWarmResumeSurfaceSnapshot();
            return false;
        }
        int orientation = getResources().getConfiguration().orientation;
        return sWarmResumeSurfaceSnapshotOrientation == android.content.res.Configuration.ORIENTATION_UNDEFINED
            || sWarmResumeSurfaceSnapshotOrientation == orientation;
    }

    private void clearWarmResumeSurfaceSnapshot() {
        sWarmResumeSurfaceSnapshot = null;
        sWarmResumeSurfaceSnapshotUptimeMs = 0;
        sWarmResumeSurfaceSnapshotOrientation = android.content.res.Configuration.ORIENTATION_UNDEFINED;
    }

    private void clearLatestSurfaceSnapshot() {
        mLatestSurfaceSnapshot = null;
        mLatestSurfaceSnapshotOrientation = android.content.res.Configuration.ORIENTATION_UNDEFINED;
    }

    private void trimSurfaceSnapshotCaches() {
        clearWarmResumeSurfaceSnapshot();
        clearLatestSurfaceSnapshot();

        if (mSurfaceSnapshotBackdrop != null) {
            mSurfaceSnapshotBackdrop.setImageBitmap(null);
            mSurfaceSnapshotBackdrop.setVisibility(View.GONE);
        }
        if (mSurfaceSnapshotOverlay != null) {
            mSurfaceSnapshotOverlay.animate().cancel();
            mSurfaceSnapshotOverlay.setImageBitmap(null);
            mSurfaceSnapshotOverlay.setAlpha(1.0f);
            mSurfaceSnapshotOverlay.setVisibility(View.GONE);
        }
        if (mSurfaceRecoveryOverlayVisible && mSurfaceCoverOverlay != null) {
            mSurfaceCoverOverlay.animate().cancel();
            mSurfaceCoverOverlay.setAlpha(1.0f);
            mSurfaceCoverOverlay.setVisibility(View.VISIBLE);
            mSurfaceCoverOverlay.bringToFront();
        }
    }

    boolean hasRecoverySnapshotAvailable() {
        return mLatestSurfaceSnapshot != null;
    }

    private boolean hasCurrentOrientationRecoverySnapshot() {
        if (mLatestSurfaceSnapshot == null) {
            return false;
        }
        int orientation = getResources().getConfiguration().orientation;
        return mLatestSurfaceSnapshotOrientation == android.content.res.Configuration.ORIENTATION_UNDEFINED
            || mLatestSurfaceSnapshotOrientation == orientation;
    }

    private void restoreWarmResumeSurfaceSnapshotIfAvailable() {
        if (!canRestoreWarmResumeSurfaceSnapshot()) {
            return;
        }
        mLatestSurfaceSnapshot = sWarmResumeSurfaceSnapshot;
        mLatestSurfaceSnapshotOrientation = getResources().getConfiguration().orientation;
        updateSurfaceSnapshotBackdrop();
        clearWarmResumeSurfaceSnapshot();
    }

    @Override
    public void onCreate(Bundle savedInstanceState) {
        if (mWebSocketsThread == null || !mWebSocketsThread.isAlive()) {
            mWebSocketsThread = new HandlerThread("WebSocketsThread");
            mWebSocketsThread.start();
            mWebSocketsHandler = new Handler(mWebSocketsThread.getLooper());
        }

        // On API 30+, Theme.NoTitleBar.Fullscreen sets FLAG_FULLSCREEN which positions
        // the window below the status bar, conflicting with the modern WindowInsetsController.
        // Switch from the launch theme to the app theme and handle fullscreen programmatically.
        if (Build.VERSION.SDK_INT >= 30) {
            setTheme(R.style.MakepadAppTheme);
        }
        
        super.onCreate(savedInstanceState);
        
        this.requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().setSoftInputMode(
            LayoutParams.SOFT_INPUT_ADJUST_NOTHING
                | LayoutParams.SOFT_INPUT_STATE_UNCHANGED
        );

        // Default state: content below system bars (status bar visible).
        // Apps that want fullscreen can request CxOsOp::FullscreenWindow which
        // calls applyFullScreen(true) to hide bars and extend content behind them.

        view = new MakepadSurface(this);
        // Put it inside a parent layout which can resize it using padding
        ResizingLayout layout = new ResizingLayout(this);
        FrameLayout surfaceContentLayout = new FrameLayout(this);
        surfaceContentLayout.setLayoutParams(new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        surfaceContentLayout.setBackgroundResource(R.drawable.makepad_launch_background);

        mSurfaceSnapshotBackdrop = new ImageView(this);
        mSurfaceSnapshotBackdrop.setLayoutParams(new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        mSurfaceSnapshotBackdrop.setBackgroundResource(R.drawable.makepad_launch_background);
        mSurfaceSnapshotBackdrop.setScaleType(ImageView.ScaleType.FIT_CENTER);
        mSurfaceSnapshotBackdrop.setClickable(false);
        mSurfaceSnapshotBackdrop.setFocusable(false);
        mSurfaceSnapshotBackdrop.setVisibility(View.GONE);
        surfaceContentLayout.addView(mSurfaceSnapshotBackdrop);

        view.setLayoutParams(new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        surfaceContentLayout.addView(view);
        layout.addView(surfaceContentLayout);

        mRootLayout = new FrameLayout(this);
        mRootLayout.addView(layout);

        mSurfaceCoverOverlay = new FrameLayout(this);
        mSurfaceCoverOverlay.setLayoutParams(new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        mSurfaceCoverOverlay.setBackgroundResource(R.drawable.makepad_launch_background);
        mSurfaceCoverOverlay.setClickable(false);
        mSurfaceCoverOverlay.setFocusable(false);
        mSurfaceCoverOverlay.setAlpha(1.0f);
        mSurfaceCoverOverlay.setVisibility(View.GONE);
        mRootLayout.addView(mSurfaceCoverOverlay);

        mSurfaceSnapshotOverlay = new ImageView(this);
        mSurfaceSnapshotOverlay.setLayoutParams(new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        mSurfaceSnapshotOverlay.setBackgroundResource(R.drawable.makepad_launch_background);
        mSurfaceSnapshotOverlay.setScaleType(ImageView.ScaleType.FIT_CENTER);
        mSurfaceSnapshotOverlay.setClickable(false);
        mSurfaceSnapshotOverlay.setFocusable(false);
        mSurfaceSnapshotOverlay.setAlpha(1.0f);
        mSurfaceSnapshotOverlay.setVisibility(View.GONE);
        mRootLayout.addView(mSurfaceSnapshotOverlay);

        mCameraPreviewOverlay = new FrameLayout(this);
        mRootLayout.addView(mCameraPreviewOverlay);

        mSelectionHandleOverlay = new FrameLayout(this);
        mSelectionHandleOverlay.setLayoutParams(new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));
        mSelectionHandleOverlay.setClickable(false);
        mSelectionHandleOverlay.setFocusable(false);
        mSelectionHandleOverlay.setVisibility(View.GONE);
        mRootLayout.addView(mSelectionHandleOverlay);

        mSelectionHandleSizePx = Math.max(24, (int) (getResources().getDisplayMetrics().density * 24.0f));
        mSelectionHandleStart = new SelectionHandleView(this, 0xFF4A90E2, mSelectionHandleSizePx);
        mSelectionHandleEnd = new SelectionHandleView(this, 0xFF4A90E2, mSelectionHandleSizePx);
        mSelectionHandleStart.setOnTouchListener(createSelectionHandleDragListener(SELECTION_HANDLE_START));
        mSelectionHandleEnd.setOnTouchListener(createSelectionHandleDragListener(SELECTION_HANDLE_END));
        mSelectionHandleOverlay.addView(mSelectionHandleStart);
        mSelectionHandleOverlay.addView(mSelectionHandleEnd);

        setContentView(mRootLayout);
        restoreWarmResumeSurfaceSnapshotIfAvailable();
        updateTaskDescription();

        MakepadNative.activityOnCreate(this);
        registerPhysicalKeyboardListener();

        mVideoPlaybackThread = new HandlerThread("VideoPlayerThread");
        mVideoPlaybackThread.start(); // TODO: only start this if its needed.
        mVideoPlaybackHandler = new Handler(mVideoPlaybackThread.getLooper());
        mVideoPlayerRunnables = new HashMap<Long, VideoPlayerRunnable>();



        String cache_path = this.getCacheDir().getAbsolutePath();
        String data_path = this.getFilesDir().getAbsolutePath();
        float density = getResources().getDisplayMetrics().density;
        boolean isEmulator = this.isEmulator();
        String androidVersion = Build.VERSION.RELEASE;
        String buildNumber = Build.DISPLAY;
        String kernelVersion = this.getKernelVersion();
        int sdkVersion = Build.VERSION.SDK_INT;

        MakepadNative.onAndroidParams(cache_path, data_path, density, isEmulator, androidVersion, buildNumber, kernelVersion);

        // Set volume keys to control music stream, we might want make this flexible for app devs
        setVolumeControlStream(AudioManager.STREAM_MUSIC);

        float refreshRate = getDeviceRefreshRate();
        MakepadNative.initChoreographer(refreshRate, sdkVersion);
        //% MAIN_ACTIVITY_ON_CREATE
        
    }

    @Override
    protected void onStart() {
        super.onStart();
        restoreSurfaceViewForWarmResumeIfNeeded();
        MakepadNative.activityOnStart();
    }

    @Override
    protected void onResume() {
        super.onResume();
        restoreSurfaceViewForWarmResumeIfNeeded();
        updateTaskDescription();
        MakepadNative.activityOnResume();
        reportPhysicalKeyboardIfChanged();

        //% MAIN_ACTIVITY_ON_RESUME
    }
    @Override
    protected void onPause() {
        prepareSurfaceSnapshotOverlayForPause();
        super.onPause();
        MakepadNative.activityOnPause();

        //% MAIN_ACTIVITY_ON_PAUSE
    }

    @Override
    protected void onStop() {
        super.onStop();
        MakepadNative.activityOnStop();
    }

    @Override
    protected void onDestroy() {
        unregisterPhysicalKeyboardListener();
        if (mCameraPreviewOverlay != null) {
            for (Long videoId : mCameraPreviewViews.keySet()) {
                MakepadNative.onCameraPreviewSurfaceDestroyed(videoId);
            }
            mCameraPreviewViews.clear();
            mCameraPreviewOverlay.removeAllViews();
        }
        if (mSelectionHandleOverlay != null) {
            mSelectionHandleOverlay.removeAllViews();
            mSelectionHandleOverlay = null;
            mSelectionHandleStart = null;
            mSelectionHandleEnd = null;
        }
        if (mSurfaceCoverOverlay != null) {
            mRootLayout.removeView(mSurfaceCoverOverlay);
            mSurfaceCoverOverlay = null;
        }
        if (mSurfaceSnapshotBackdrop != null) {
            mSurfaceSnapshotBackdrop.setImageBitmap(null);
            mSurfaceSnapshotBackdrop = null;
        }
        if (mSurfaceSnapshotOverlay != null) {
            mSurfaceSnapshotOverlay.setImageBitmap(null);
            mRootLayout.removeView(mSurfaceSnapshotOverlay);
            mSurfaceSnapshotOverlay = null;
        }
        clearLatestSurfaceSnapshot();
        mSurfaceSnapshotCopyInFlight = false;
        cleanupVideoPlaybackState();
        shutdownVideoPlaybackThread();
        if (!mIsSwitchingActivity) {
            cleanupNetworkState();
            shutdownWebSocketsThread();
        }
        super.onDestroy();
        MakepadNative.activityOnDestroy();
    }

    @Override
    public void onTrimMemory(int level) {
        super.onTrimMemory(level);
        switch (level) {
            case ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE:
            case ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW:
            case ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL:
            case ComponentCallbacks2.TRIM_MEMORY_BACKGROUND:
            case ComponentCallbacks2.TRIM_MEMORY_MODERATE:
            case ComponentCallbacks2.TRIM_MEMORY_COMPLETE:
                trimSurfaceSnapshotCaches();
                break;
            default:
                break;
        }
    }

    @Override
    public void onLowMemory() {
        super.onLowMemory();
        trimSurfaceSnapshotCaches();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        super.onBackPressed();
        MakepadNative.onBackPressed();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        MakepadNative.activityOnWindowFocusChanged(hasFocus);
    }

    @Override
    protected void onUserLeaveHint() {
        prepareSurfaceSnapshotOverlayForPause();
        super.onUserLeaveHint();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        restoreSurfaceViewForWarmResumeIfNeeded();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        //% MAIN_ACTIVITY_ON_ACTIVITY_RESULT
    }

    @Override
    public void onRequestPermissionsResult(int requestId, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestId, permissions, grantResults);

        for (int i = 0; i < permissions.length; i++) {
            int status;
            if (grantResults[i] == PackageManager.PERMISSION_GRANTED) {
                status = 1; // Granted
            } else {
                // Permission denied - check if we can ask again
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M && shouldShowRequestPermissionRationale(permissions[i])) {
                    status = 2; // DeniedCanRetry (can show rationale and retry)
                } else {
                    status = 3; // DeniedPermanent (user selected "Don't ask again" or hit limit)
                }
            }
            
            // Use the new unified callback
            MakepadNative.onPermissionResult(permissions[i], requestId, status);
        }
    }

    public int checkPermission(String permission) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            if (checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) {
                return 1; // Granted
            } else {
                // Check if permission was previously denied
                if (shouldShowRequestPermissionRationale(permission)) {
                    return 2; // DeniedCanRetry (user previously declined but can show rationale)
                } else {
                    // This could be either:
                    // - NotDetermined (never asked before) 
                    // - DeniedPermanent (user selected "Don't ask again" or hit Android 11+ limit)
                    // We return 0 for NotDetermined as the safest assumption - let the app request and find out
                    return 0; // NotDetermined (assume we can still ask)
                }
            }
        } else {
            // Permissions are granted at install time on older Android versions
            return 1; // Granted
        }
    }

    public void requestPermission(String permission, int requestId) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            if (checkSelfPermission(permission) != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[]{permission}, requestId);
            } else {
                // Permission already granted
                MakepadNative.onPermissionResult(permission, requestId, 1); // 1 = Granted
            }
        } else {
            // Permissions are granted at install time on older Android versions
            MakepadNative.onPermissionResult(permission, requestId, 1); // 1 = Granted
        }
    }

    @SuppressWarnings("deprecation")
    public void setFullScreen(final boolean fullscreen) {
        runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    applyFullScreen(fullscreen);
                }
            });
    }

    // Tints the system bar (status/navigation bar) icons and text. A "light"
    // system bar has a light background, so it needs dark icons for contrast;
    // we therefore request dark icons when the app's background is light.
    public void setSystemBarAppearance(final boolean darkIcons) {
        runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    mSystemBarDarkIcons = darkIcons;
                    applySystemBarAppearance();
                }
            });
    }

    // Applies the currently desired system-bar icon tint (mSystemBarDarkIcons)
    // to the window. Safe to call repeatedly. It is also re-invoked from
    // applyFullScreen(), because the legacy (pre-API-30) fullscreen path
    // rewrites the whole systemUiVisibility bitmask and would otherwise drop
    // the light-status/navigation-bar bits.
    @SuppressWarnings("deprecation")
    private void applySystemBarAppearance() {
        Window window = getWindow();
        if (window == null) {
            return;
        }
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController controller = window.getInsetsController();
            if (controller != null) {
                int mask = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                controller.setSystemBarsAppearance(mSystemBarDarkIcons ? mask : 0, mask);
            }
        } else {
            View decorView = window.getDecorView();
            int lightBars = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR
                | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
            int flags = decorView.getSystemUiVisibility();
            if (mSystemBarDarkIcons) {
                flags |= lightBars;
            } else {
                flags &= ~lightBars;
            }
            decorView.setSystemUiVisibility(flags);
        }
    }

    private boolean canCaptureSurfaceSnapshot() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return false;
        }
        if (view == null || view.getWidth() <= 0 || view.getHeight() <= 0) {
            return false;
        }
        Surface surface = view.getHolder().getSurface();
        return surface != null && surface.isValid();
    }

    private void refreshSurfaceSnapshotCache() {
        if (!canCaptureSurfaceSnapshot() || mSurfaceSnapshotCopyInFlight) {
            return;
        }
        mSurfaceSnapshotCopyInFlight = true;

        final Bitmap snapshot = Bitmap.createBitmap(
            view.getWidth(),
            view.getHeight(),
            Bitmap.Config.ARGB_8888
        );
        PixelCopy.request(view, snapshot, copyResult -> {
            mSurfaceSnapshotCopyInFlight = false;
            if (copyResult != PixelCopy.SUCCESS) {
                return;
            }
            mLatestSurfaceSnapshot = snapshot;
            mLatestSurfaceSnapshotOrientation = getResources().getConfiguration().orientation;
            cacheWarmResumeSurfaceSnapshot(snapshot);
            updateSurfaceSnapshotBackdrop();
            if (mSurfaceRecoveryOverlayVisible) {
                showSurfaceRecoverySnapshotIfAvailable();
            }
        }, mHandler);
    }

    private void prepareSurfaceSnapshotOverlayForPause() {
        if (mSurfaceRecoveryOverlayVisible && view != null && view.getVisibility() != View.VISIBLE) {
            return;
        }
        if (!hasRecoverySnapshotAvailable()) {
            refreshSurfaceSnapshotCache();
            return;
        }
        mSurfaceRecoveryOverlayVisible = true;
        cacheWarmResumeSurfaceSnapshot(mLatestSurfaceSnapshot);
        updateSurfaceSnapshotBackdrop();
        if (view != null) {
            view.setVisibility(View.INVISIBLE);
        }
        showSurfaceRecoverySnapshotIfAvailable();
        refreshSurfaceSnapshotCache();
    }

    private void restoreSurfaceViewForWarmResumeIfNeeded() {
        if (!mSurfaceRecoveryOverlayVisible || view == null) {
            return;
        }

        Surface surface = view.getHolder().getSurface();
        boolean surfaceValid = surface != null && surface.isValid();
        if (view.getVisibility() == View.VISIBLE && surfaceValid) {
            return;
        }

        // Keep the recovery overlay visible, but restore the SurfaceView itself
        // so Android can recreate its surface on same-activity warm resumes.
        view.setVisibility(View.VISIBLE);
        if (!showSurfaceRecoverySnapshotIfAvailable() && mSurfaceCoverOverlay != null) {
            mSurfaceCoverOverlay.setAlpha(1.0f);
            mSurfaceCoverOverlay.setVisibility(View.VISIBLE);
            mSurfaceCoverOverlay.bringToFront();
        }
        updateSurfaceSnapshotBackdrop();
    }

    private Bitmap createTaskDescriptionIconBitmap() {
        int iconResId = getApplicationIconResId();
        if (iconResId == 0) {
            return null;
        }
        Drawable drawable = getDrawable(iconResId);
        if (drawable == null) {
            return null;
        }
        int width = Math.max(1, drawable.getIntrinsicWidth());
        int height = Math.max(1, drawable.getIntrinsicHeight());
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        drawable.setBounds(0, 0, width, height);
        drawable.draw(canvas);
        return bitmap;
    }

    @SuppressWarnings("deprecation")
    private void updateTaskDescription() {
        try {
            String label = getApplicationName();
            int iconResId = getApplicationIconResId();
            ActivityManager.TaskDescription taskDescription;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                taskDescription = new ActivityManager.TaskDescription.Builder()
                    .setLabel(label)
                    .setIcon(iconResId)
                    .setPrimaryColor(TASK_DESCRIPTION_BACKGROUND_COLOR)
                    .setBackgroundColor(TASK_DESCRIPTION_BACKGROUND_COLOR)
                    .build();
            }
            else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                taskDescription = new ActivityManager.TaskDescription(
                    label,
                    iconResId,
                    TASK_DESCRIPTION_BACKGROUND_COLOR
                );
            }
            else {
                taskDescription = new ActivityManager.TaskDescription(
                    label,
                    createTaskDescriptionIconBitmap(),
                    TASK_DESCRIPTION_BACKGROUND_COLOR
                );
            }
            setTaskDescription(taskDescription);
        }
        catch (Throwable throwable) {
            Log.w(LOG_TAG, "Failed to update task description", throwable);
        }
    }

    private void updateSurfaceSnapshotBackdrop() {
        if (mSurfaceSnapshotBackdrop == null) {
            return;
        }

        if (hasCurrentOrientationRecoverySnapshot()) {
            mSurfaceSnapshotBackdrop.setImageBitmap(mLatestSurfaceSnapshot);
            mSurfaceSnapshotBackdrop.setVisibility(View.VISIBLE);
            return;
        }

        mSurfaceSnapshotBackdrop.setImageBitmap(null);
        mSurfaceSnapshotBackdrop.setVisibility(View.GONE);
    }

    private boolean showSurfaceRecoverySnapshotIfAvailable() {
        if (mSurfaceSnapshotOverlay == null || mSurfaceCoverOverlay == null) {
            return false;
        }

        if (hasCurrentOrientationRecoverySnapshot()) {
            mSurfaceSnapshotOverlay.setImageBitmap(mLatestSurfaceSnapshot);
            mSurfaceSnapshotOverlay.setAlpha(1.0f);
            mSurfaceSnapshotOverlay.setVisibility(View.VISIBLE);
            mSurfaceSnapshotOverlay.bringToFront();
            mSurfaceCoverOverlay.setVisibility(View.GONE);
            mSurfaceCoverOverlay.setAlpha(1.0f);
            return true;
        }

        mSurfaceSnapshotOverlay.setImageBitmap(null);
        mSurfaceSnapshotOverlay.setVisibility(View.GONE);
        return false;
    }

    private void applySurfaceCoverVisibility(boolean visible) {
        if (mSurfaceCoverOverlay == null || mSurfaceSnapshotOverlay == null) {
            return;
        }

        boolean wasRecoveryOverlayVisible = mSurfaceRecoveryOverlayVisible;
        if (visible && !hasRecoverySnapshotAvailable()) {
            mSurfaceRecoveryOverlayVisible = true;
            if (view != null) {
                view.setVisibility(View.INVISIBLE);
            }
            mSurfaceCoverOverlay.animate().cancel();
            mSurfaceSnapshotOverlay.animate().cancel();
            mSurfaceSnapshotOverlay.setImageBitmap(null);
            mSurfaceSnapshotOverlay.setVisibility(View.GONE);
            mSurfaceSnapshotOverlay.setAlpha(1.0f);
            mSurfaceCoverOverlay.setAlpha(1.0f);
            mSurfaceCoverOverlay.setVisibility(View.VISIBLE);
            mSurfaceCoverOverlay.bringToFront();
            if (!wasRecoveryOverlayVisible) {
                refreshSurfaceSnapshotCache();
            }
            return;
        }
        mSurfaceRecoveryOverlayVisible = visible;
        if (view != null) {
            view.setVisibility(visible ? View.INVISIBLE : view.getVisibility());
        }
        if (visible && !wasRecoveryOverlayVisible) {
            refreshSurfaceSnapshotCache();
        }

        mSurfaceCoverOverlay.animate().cancel();
        mSurfaceSnapshotOverlay.animate().cancel();
        if (visible) {
            if (showSurfaceRecoverySnapshotIfAvailable()) {
                return;
            }
            mSurfaceCoverOverlay.setAlpha(1.0f);
            mSurfaceCoverOverlay.setVisibility(View.VISIBLE);
            mSurfaceCoverOverlay.bringToFront();
            return;
        }

        if (mSurfaceCoverOverlay.getVisibility() != View.VISIBLE
            && mSurfaceSnapshotOverlay.getVisibility() != View.VISIBLE) {
            mSurfaceCoverOverlay.setAlpha(1.0f);
            mSurfaceSnapshotOverlay.setAlpha(1.0f);
            if (view != null) {
                view.setVisibility(View.VISIBLE);
            }
            clearWarmResumeSurfaceSnapshot();
            return;
        }

        final FrameLayout surfaceCoverOverlay = mSurfaceCoverOverlay;
        final ImageView surfaceSnapshotOverlay = mSurfaceSnapshotOverlay;
        if (surfaceSnapshotOverlay.getVisibility() == View.VISIBLE) {
            surfaceCoverOverlay.setVisibility(View.GONE);
            surfaceCoverOverlay.setAlpha(1.0f);
            surfaceSnapshotOverlay.animate()
                .alpha(0.0f)
                .setDuration(SURFACE_COVER_FADE_OUT_MS)
                .withEndAction(new Runnable() {
                    @Override
                    public void run() {
                        if (mSurfaceCoverOverlay != surfaceCoverOverlay
                            || mSurfaceSnapshotOverlay != surfaceSnapshotOverlay) {
                            return;
                        }
                        surfaceSnapshotOverlay.setVisibility(View.GONE);
                        surfaceSnapshotOverlay.setAlpha(1.0f);
                        if (view != null) {
                            view.setVisibility(View.VISIBLE);
                        }
                        clearWarmResumeSurfaceSnapshot();
                    }
                });
            return;
        }

        surfaceCoverOverlay.animate()
            .alpha(0.0f)
            .setDuration(SURFACE_COVER_FADE_OUT_MS)
            .withEndAction(new Runnable() {
                @Override
                public void run() {
                    if (mSurfaceCoverOverlay != surfaceCoverOverlay) {
                        return;
                    }
                    surfaceCoverOverlay.setVisibility(View.GONE);
                    surfaceCoverOverlay.setAlpha(1.0f);
                    if (view != null) {
                        view.setVisibility(View.VISIBLE);
                    }
                    clearWarmResumeSurfaceSnapshot();
                }
            });
    }

    public void setSurfaceCoverVisible(final boolean visible) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            applySurfaceCoverVisibility(visible);
            return;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                applySurfaceCoverVisibility(visible);
            }
        });
    }

    public void requestSurfaceSnapshotRefresh() {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            refreshSurfaceSnapshotCache();
            return;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                refreshSurfaceSnapshotCache();
            }
        });
    }

    @SuppressWarnings("deprecation")
    private void applyFullScreen(boolean fullscreen) {
        View decorView = getWindow().getDecorView();

        if (fullscreen) {
            // WindowManager.LayoutParams.layoutInDisplayCutoutMode is API 28+
            // (display cutouts didn't exist before Android 9). Touching the
            // field at all on API 26-27 throws NoSuchFieldError, so guard it.
            // LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS = 3 is API 30+; on 28-29 we
            // fall back to SHORT_EDGES.
            if (Build.VERSION.SDK_INT >= 28) {
                getWindow().getAttributes().layoutInDisplayCutoutMode =
                    Build.VERSION.SDK_INT >= 30 ? 3 : LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            }
            if (Build.VERSION.SDK_INT >= 30) {
                getWindow().setDecorFitsSystemWindows(false);
                android.view.WindowInsetsController controller = getWindow().getInsetsController();
                if (controller != null) {
                    controller.hide(WindowInsets.Type.statusBars() | WindowInsets.Type.navigationBars());
                    // BEHAVIOR_SHOW_TRANSIENT_BARS_BY_GESTURE = 2
                    controller.setSystemBarsBehavior(2);
                }
            } else {
                int uiOptions = View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY;
                decorView.setSystemUiVisibility(uiOptions);
            }
        }
        else {
            if (Build.VERSION.SDK_INT >= 30) {
                getWindow().setDecorFitsSystemWindows(true);
                android.view.WindowInsetsController controller = getWindow().getInsetsController();
                if (controller != null) {
                    controller.show(WindowInsets.Type.statusBars() | WindowInsets.Type.navigationBars());
                }
            } else {
                decorView.setSystemUiVisibility(0);
            }
        }

        // The legacy (pre-API-30) branches above replace the entire
        // systemUiVisibility bitmask, so re-assert the system-bar icon tint
        // on top of the new flags. On API 30+ this is an independent,
        // idempotent re-apply.
        applySystemBarAppearance();

        // Force a layout pass so the SurfaceView gets the new dimensions
        if (view != null) {
            view.requestLayout();
        }
    }
    
    public void switchActivityClass(Class c){
        mIsSwitchingActivity = true;
        Intent intent = new Intent(getApplicationContext(), c);
        Intent currentIntent = getIntent();
        if (currentIntent != null && currentIntent.getExtras() != null) {
            intent.putExtras(currentIntent.getExtras());
        }
        startActivity(intent);
        finish();
    }

    private void cleanupVideoPlaybackState() {
        if (mVideoPlayerRunnables != null) {
            ArrayList<Long> videoIds = new ArrayList<>(mVideoPlayerRunnables.keySet());
            for (Long videoId : videoIds) {
                cleanupVideoPlaybackResources(videoId);
            }
            mVideoPlayerRunnables.clear();
        }
        if (mVideoPlaybackHandler != null) {
            mVideoPlaybackHandler.removeCallbacksAndMessages(null);
        }
    }

    private void shutdownVideoPlaybackThread() {
        if (mVideoPlaybackThread == null) {
            mVideoPlaybackHandler = null;
            return;
        }
        mVideoPlaybackThread.quitSafely();
        try {
            mVideoPlaybackThread.join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        mVideoPlaybackThread = null;
        mVideoPlaybackHandler = null;
    }

    private static void cleanupNetworkState() {
        for (MakepadWebSocket socket : new ArrayList<>(mActiveWebsockets.values())) {
            if (socket != null) {
                socket.closeSocketAndClearCallback();
            }
        }
        mActiveWebsockets.clear();
        mActiveWebsocketsReaders.clear();

        for (MakepadSocketStream socket : new ArrayList<>(mActiveSocketStreams.values())) {
            if (socket != null) {
                socket.close();
            }
        }
        mActiveSocketStreams.clear();

        if (mWebSocketsHandler != null) {
            mWebSocketsHandler.removeCallbacksAndMessages(null);
        }
    }

    private static void shutdownWebSocketsThread() {
        if (mWebSocketsThread == null) {
            mWebSocketsHandler = null;
            return;
        }
        mWebSocketsThread.quitSafely();
        try {
            mWebSocketsThread.join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        mWebSocketsThread = null;
        mWebSocketsHandler = null;
    }
    
    // Configure keyboard settings before showing - called from Rust
    public void configureKeyboard(final int keyboardType, final int autocapitalize,
                                   final int autocorrect, final int returnKeyType,
                                   final boolean isMultiline, final boolean isSecure) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (view != null) {
                    view.configureKeyboard(keyboardType, autocapitalize, autocorrect,
                                          returnKeyType, isMultiline, isSecure);
                }
            }
        });
    }

    public void showKeyboard(final boolean show) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (show) {
                    if (view == null || view.getInputMode() == MakepadSurface.INPUT_MODE_NONE) {
                        return;
                    }
                    // The IME only shows for the view that currently holds
                    // focus and is "served" by the InputMethodManager. The
                    // SurfaceView can end up not focused (window-focus churn,
                    // surface re-creation, returning from another activity,
                    // etc.); after that, showSoftInput() is silently ignored —
                    // logcat shows "Ignoring showSoftInput() as view=... is
                    // not served". Re-focus the SurfaceView before every show
                    // so it becomes the served editor. This is the canonical
                    // precondition for showSoftInput(); the previous code
                    // relied on the view simply staying focused from the
                    // one-time requestFocus() in the MakepadSurface
                    // constructor, which is not guaranteed.
                    view.requestFocus();
                    InputMethodManager imm = (InputMethodManager)getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.showSoftInput(view, 0);
                } else {
                    // Hiding the IME via the legacy InputMethodManager
                    // .hideSoftInputFromWindow() is unreliable on modern Android:
                    // with an edge-to-edge window (targetSdk 35) and on
                    // OEM-customized builds (e.g. OxygenOS / OnePlus) the request
                    // is silently dropped and the keyboard stays up. The
                    // WindowInsetsController.hide(ime()) path is the canonical
                    // API 30+ way, and matches how this app already drives the
                    // system bars and reads IME insets.
                    if (Build.VERSION.SDK_INT >= 30) {
                        android.view.WindowInsetsController controller = getWindow().getInsetsController();
                        if (controller != null) {
                            controller.hide(WindowInsets.Type.ime());
                        }
                    } else {
                        InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
                        if (imm != null && view != null) {
                            imm.hideSoftInputFromWindow(view.getWindowToken(), 0);
                        }
                    }
                }
            }
        });
    }

    // Update IME text state for programmatic changes - called from Rust
    // Note: This should only be called for programmatic text changes (e.g., clear button),
    // NOT during normal IME input (which flows Java to Rust via onImeTextStateChanged)
    public void updateImeTextState(final String fullText, final int selStart, final int selEnd,
                                   final int composingStart, final int composingEnd) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (view != null) {
                    view.updateImeTextState(
                        fullText,
                        selStart,
                        selEnd,
                        composingStart,
                        composingEnd
                    );
                }
            }
        });
    }

    public void copyToClipboard(String content) {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        // User-facing description of the clipboard content
        String clipLabel = getApplicationName() + " clip";
        ClipData clip = ClipData.newPlainText(clipLabel, content);
        clipboard.setPrimaryClip(clip);
    }

    public String pasteFromClipboard() {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard.hasPrimaryClip()) {
            ClipData clipData = clipboard.getPrimaryClip();
            if (clipData != null && clipData.getItemCount() > 0) {
                ClipData.Item item = clipData.getItemAt(0);
                CharSequence text = item.getText();
                if (text != null) {
                    return text.toString();
                }
            }
        }
        return "";
    }

    private String getApplicationName() {
        ApplicationInfo applicationInfo = getApplicationContext().getApplicationInfo();
        CharSequence appName = applicationInfo.loadLabel(getPackageManager());
        return appName.toString();
    }

    private int getApplicationIconResId() {
        ApplicationInfo applicationInfo = getApplicationContext().getApplicationInfo();
        return applicationInfo.icon;
    }

    public void showClipboardActions(final boolean hasSelection, final int left, final int top, final int right, final int bottom, final int keyboardShift) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                mHasSelection = hasSelection;
                mSelectionBounds[0] = left;
                mSelectionBounds[1] = top;
                mSelectionBounds[2] = right;
                mSelectionBounds[3] = bottom;
                mKeyboardShift = keyboardShift;

                // If ActionMode is already showing, finish it first
                if (mActionMode != null) {
                    mActionMode.finish();
                }

                // Start ActionMode with our callback
                // Use TYPE_FLOATING (API 23+) to show near finger, falls back to primary for older versions
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    mActionMode = startActionMode(new ActionMode.Callback2() {
                        @Override
                        public boolean onCreateActionMode(ActionMode mode, Menu menu) {
                            return onCreateActionModeInternal(mode, menu);
                        }

                        @Override
                        public boolean onPrepareActionMode(ActionMode mode, Menu menu) {
                            return onPrepareActionModeInternal(mode, menu);
                        }

                        @Override
                        public boolean onActionItemClicked(ActionMode mode, MenuItem item) {
                            return onActionItemClickedInternal(mode, item);
                        }

                        @Override
                        public void onDestroyActionMode(ActionMode mode) {
                            onDestroyActionModeInternal(mode);
                        }

                        @Override
                        public void onGetContentRect(ActionMode mode, View view, android.graphics.Rect outRect) {
                            // The content rect tells Android what area to AVOID covering (not where to position)
                            // Android's FloatingToolbar will automatically position itself above or below this rect
                            // based on available screen space

                            // Use asymmetric padding: more above (for better spacing when popup appears above),
                            // less below (already looks good), and some on sides for visual balance
                            int topPadding = 16;      // More padding above pushes popup higher
                            int bottomPadding = 2;    // Minimal padding below (already good spacing)
                            int sidePadding = 2;      // Horizontal padding for visual balance

                            int left = mSelectionBounds[0] - sidePadding;
                            int top = mSelectionBounds[1] - topPadding;
                            int right = mSelectionBounds[2] + sidePadding;
                            int bottom = mSelectionBounds[3] + bottomPadding;

                            outRect.set(left, top, right, bottom);
                        }
                    }, ActionMode.TYPE_FLOATING);
                } else {
                    mActionMode = startActionMode(new ActionMode.Callback() {
                        @Override
                        public boolean onCreateActionMode(ActionMode mode, Menu menu) {
                            return onCreateActionModeInternal(mode, menu);
                        }

                        @Override
                        public boolean onPrepareActionMode(ActionMode mode, Menu menu) {
                            return onPrepareActionModeInternal(mode, menu);
                        }

                        @Override
                        public boolean onActionItemClicked(ActionMode mode, MenuItem item) {
                            return onActionItemClickedInternal(mode, item);
                        }

                        @Override
                        public void onDestroyActionMode(ActionMode mode) {
                            onDestroyActionModeInternal(mode);
                        }
                    });
                }
            }
        });
    }

    public void dismissClipboardActions() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (mActionMode != null) {
                    mActionMode.finish();
                    mActionMode = null;
                }
            }
        });
    }

    // Helper methods for ActionMode callbacks (shared between Callback and Callback2)
    private boolean onCreateActionModeInternal(ActionMode mode, Menu menu) {
        // Add menu items: Copy, Cut, Paste, Select All
        menu.add(0, android.R.id.copy, 0, android.R.string.copy);
        menu.add(0, android.R.id.cut, 0, android.R.string.cut);
        menu.add(0, android.R.id.paste, 0, android.R.string.paste);
        menu.add(0, android.R.id.selectAll, 0, android.R.string.selectAll);
        return true;
    }

    private boolean onPrepareActionModeInternal(ActionMode mode, Menu menu) {
        boolean hasSelection = mHasSelection;
        boolean hasClipboard = false;

        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard.hasPrimaryClip()) {
            hasClipboard = true;
        }

        MenuItem copyItem = menu.findItem(android.R.id.copy);
        MenuItem cutItem = menu.findItem(android.R.id.cut);
        MenuItem pasteItem = menu.findItem(android.R.id.paste);

        if (copyItem != null) copyItem.setVisible(hasSelection);
        if (cutItem != null) cutItem.setVisible(hasSelection);
        if (pasteItem != null) pasteItem.setVisible(hasClipboard);

        return true;
    }

    private boolean onActionItemClickedInternal(ActionMode mode, MenuItem item) {
        int id = item.getItemId();

        if (id == android.R.id.copy) {
            MakepadNative.onClipboardAction("copy");
            mode.finish();
            return true;
        } else if (id == android.R.id.cut) {
            MakepadNative.onClipboardAction("cut");
            mode.finish();
            return true;
        } else if (id == android.R.id.paste) {
            String content = pasteFromClipboard();
            MakepadNative.onClipboardPaste(content);
            mode.finish();
            return true;
        } else if (id == android.R.id.selectAll) {
            MakepadNative.onClipboardAction("select_all");
            // Sync Java-side selection with Rust so backspace/delete will work
            // This updates mEditable's selection and notifies the IME
            view.selectAllInEditable();
            mode.finish();
            return true;
        }
        return false;
    }

    private void onDestroyActionModeInternal(ActionMode mode) {
        mActionMode = null;
        mHasSelection = false;
    }

    private View.OnTouchListener createSelectionHandleDragListener(final int handleKind) {
        return new View.OnTouchListener() {
            private final int[] rootLocation = new int[2];

            @Override
            public boolean onTouch(View v, MotionEvent event) {
                if (mRootLayout == null) {
                    return false;
                }
                mRootLayout.getLocationOnScreen(rootLocation);
                float absX = event.getRawX() - rootLocation[0];
                float absY = event.getRawY() - rootLocation[1];
                int action = event.getActionMasked();

                if (action == MotionEvent.ACTION_DOWN) {
                    setSelectionHandlePosition(v, absX, absY);
                    MakepadNative.onSelectionHandleDrag(handleKind, SELECTION_DRAG_BEGIN, absX, absY, event.getEventTime());
                    return true;
                }
                if (action == MotionEvent.ACTION_MOVE) {
                    setSelectionHandlePosition(v, absX, absY);
                    MakepadNative.onSelectionHandleDrag(handleKind, SELECTION_DRAG_MOVE, absX, absY, event.getEventTime());
                    return true;
                }
                if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
                    setSelectionHandlePosition(v, absX, absY);
                    MakepadNative.onSelectionHandleDrag(handleKind, SELECTION_DRAG_END, absX, absY, event.getEventTime());
                    return true;
                }
                return false;
            }
        };
    }

    private void setSelectionHandlePosition(View handle, float x, float y) {
        if (handle == null) {
            return;
        }
        handle.setX(x - (mSelectionHandleSizePx * 0.5f));
        handle.setY(y - (mSelectionHandleSizePx * 0.5f));
    }

    public void showSelectionHandles(final float startX, final float startY, final float endX, final float endY) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (mSelectionHandleOverlay == null || mSelectionHandleStart == null || mSelectionHandleEnd == null) {
                    return;
                }
                mSelectionHandleOverlay.setVisibility(View.VISIBLE);
                setSelectionHandlePosition(mSelectionHandleStart, startX, startY);
                setSelectionHandlePosition(mSelectionHandleEnd, endX, endY);
                mSelectionHandleStart.setVisibility(View.VISIBLE);
                mSelectionHandleEnd.setVisibility(View.VISIBLE);
                mSelectionHandleOverlay.bringToFront();
            }
        });
    }

    public void updateSelectionHandles(final float startX, final float startY, final float endX, final float endY) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (mSelectionHandleOverlay == null || mSelectionHandleStart == null || mSelectionHandleEnd == null) {
                    return;
                }
                mSelectionHandleOverlay.setVisibility(View.VISIBLE);
                setSelectionHandlePosition(mSelectionHandleStart, startX, startY);
                setSelectionHandlePosition(mSelectionHandleEnd, endX, endY);
                mSelectionHandleStart.setVisibility(View.VISIBLE);
                mSelectionHandleEnd.setVisibility(View.VISIBLE);
            }
        });
    }

    public void hideSelectionHandles() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (mSelectionHandleOverlay == null || mSelectionHandleStart == null || mSelectionHandleEnd == null) {
                    return;
                }
                mSelectionHandleStart.setVisibility(View.GONE);
                mSelectionHandleEnd.setVisibility(View.GONE);
                mSelectionHandleOverlay.setVisibility(View.GONE);
            }
        });
    }

    public void requestHttp(long id, long metadataId, String url, String method, String headers, byte[] body) {
        try {
            MakepadNetwork network = new MakepadNetwork();

            CompletableFuture<HttpResponse> future = network.performHttpRequest(url, method, headers, body);

            future.thenAccept(response -> {
                runOnUiThread(() -> MakepadNative.onHttpResponse(id, metadataId, response.getStatusCode(), response.getHeaders(), response.getBody()));
            }).exceptionally(ex -> {
                runOnUiThread(() -> MakepadNative.onHttpRequestError(id, metadataId, ex.toString()));
                return null;
            });
        } catch (Exception e) {
            MakepadNative.onHttpRequestError(id, metadataId, e.toString());
        }
    }

    public void openWebSocket(long id, String url, long callback) {
        MakepadWebSocket webSocket = new MakepadWebSocket(id, url, callback);
        mActiveWebsockets.put(id, webSocket);
        webSocket.connect();

        if (webSocket.isConnected()) {
            MakepadWebSocketReader reader = new MakepadWebSocketReader(this, webSocket);
            mWebSocketsHandler.post(reader);
            mActiveWebsocketsReaders.put(id, reader);
        } else {
            Log.e("Makepad", "openWebSocket failed id=" + id + " url=" + url);
        }
    }

    public void sendWebSocketMessage(long id, byte[] message) {
      
        MakepadWebSocket webSocket = mActiveWebsockets.get(id);
        if (webSocket != null) {
            webSocket.sendMessage(message);
        }
    }

    public void closeWebSocket(long id) {
        
        MakepadWebSocket socket = mActiveWebsockets.get(id);
        if (socket != null) {
            socket.closeSocketAndClearCallback();
        }
        MakepadWebSocketReader reader = mActiveWebsocketsReaders.get(id);
        if (reader != null) {
            mWebSocketsHandler.removeCallbacks(reader);
        }
        
        mActiveWebsocketsReaders.remove(id);
        mActiveWebsockets.remove(id);
    }

    public boolean openSocketStream(long id, String host, int port, boolean useTls, boolean ignoreSslCert) {
        MakepadSocketStream socket = new MakepadSocketStream();
        if (!socket.connect(host, port, useTls, ignoreSslCert)) {
            return false;
        }
        mActiveSocketStreams.put(id, socket);
        return true;
    }

    public byte[] socketStreamRead(long id, int maxBytes) {
        MakepadSocketStream socket = mActiveSocketStreams.get(id);
        if (socket == null) {
            return null;
        }
        return socket.read(maxBytes);
    }

    public int socketStreamWrite(long id, byte[] message) {
        MakepadSocketStream socket = mActiveSocketStreams.get(id);
        if (socket == null) {
            return -1;
        }
        return socket.write(message);
    }

    public void socketStreamSetReadTimeout(long id, int timeoutMs) {
        MakepadSocketStream socket = mActiveSocketStreams.get(id);
        if (socket != null) {
            socket.setReadTimeout(timeoutMs);
        }
    }

    public void socketStreamSetWriteTimeout(long id, int timeoutMs) {
        MakepadSocketStream socket = mActiveSocketStreams.get(id);
        if (socket != null) {
            socket.setWriteTimeout(timeoutMs);
        }
    }

    public void closeSocketStream(long id) {
        MakepadSocketStream socket = mActiveSocketStreams.get(id);
        if (socket != null) {
            socket.close();
            mActiveSocketStreams.remove(id);
        }
    }

    public void webSocketConnectionDone(long id, long callback) {
        mActiveWebsockets.remove(id);
        MakepadNative.onWebSocketClosed(callback);
    }

    public String[] getAudioDevices(long flag){
        try{

            AudioManager am = (AudioManager)this.getSystemService(Context.AUDIO_SERVICE);
            AudioDeviceInfo[] devices = null;
            ArrayList<String> out = new ArrayList<String>();
            if(flag == 0){
                devices = am.getDevices(AudioManager.GET_DEVICES_INPUTS);
            }
            else{
                devices = am.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
            }
            for(AudioDeviceInfo device: devices){
                int[] channel_counts = device.getChannelCounts();
                for(int cc: channel_counts){
                    out.add(String.format(
                        "%d$$%d$$%d$$%s",
                        device.getId(),
                        device.getType(),
                        cc,
                        device.getProductName().toString()
                    ));
                }
            }
            return out.toArray(new String[0]);
        }
        catch(Exception e){
            Log.e("Makepad", "exception: " + e.getMessage());
            Log.e("Makepad", "exception: " + e.toString());
            return null;
        }
    }

    @SuppressWarnings("deprecation")
    public void openAllMidiDevices(long delay){
        Runnable runnable = () -> {
            try{
                BluetoothManager bm = (BluetoothManager) this.getSystemService(Context.BLUETOOTH_SERVICE);
                BluetoothAdapter ba = bm.getAdapter();
                Set<BluetoothDevice> bluetooth_devices = ba.getBondedDevices();
                ArrayList<String> bt_names = new ArrayList<String>();
                MidiManager mm = (MidiManager)this.getSystemService(Context.MIDI_SERVICE);
                for(BluetoothDevice device: bluetooth_devices){
                    if(device.getType() == BluetoothDevice.DEVICE_TYPE_LE){
                        String name =device.getName();
                        bt_names.add(name);
                        mm.openBluetoothDevice(device, this, mHandler);
                    }
                }
                // this appears to give you nonworking BLE midi devices. So we skip those by name (not perfect but ok)
                for (MidiDeviceInfo info : mm.getDevices()){
                    String name = info.getProperties().getCharSequence(MidiDeviceInfo.PROPERTY_NAME).toString();
                    boolean found = false;
                    for (String bt_name : bt_names){
                        if (bt_name.equals(name)){
                            found = true;
                            break;
                        }
                    }
                    if(!found){
                        mm.openDevice(info, this, mHandler);
                    }
                }
            }
            catch(Exception e){
                Log.e("Makepad", "exception: " + e.getMessage());
                Log.e("Makepad", "exception: " + e.toString());
            }
        };
        if(delay != 0){
            mHandler.postDelayed(runnable, delay);
        }
        else{ // run now
            runnable.run();
        }
    }

    public void onDeviceOpened(MidiDevice device) {
        if(device == null){
            return;
        }
        MidiDeviceInfo info = device.getInfo();
        if(info != null){
            String name = info.getProperties().getCharSequence(MidiDeviceInfo.PROPERTY_NAME).toString();
            MakepadNative.onMidiDeviceOpened(name, device);
        }
    }

    public void attachCameraNativePreview(final long videoId, final int left, final int top, final int right, final int bottom) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (mCameraPreviewOverlay == null) {
                    return;
                }
                CameraPreviewSurface preview = mCameraPreviewViews.get(videoId);
                if (preview == null) {
                    preview = new CameraPreviewSurface(MakepadActivity.this, videoId);
                    mCameraPreviewViews.put(videoId, preview);
                    mCameraPreviewOverlay.addView(preview);
                }
                FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                    Math.max(1, right - left),
                    Math.max(1, bottom - top)
                );
                lp.leftMargin = left;
                lp.topMargin = top;
                preview.setLayoutParams(lp);
                preview.setVisibility(View.VISIBLE);
            }
        });
    }

    public void updateCameraNativePreview(final long videoId, final int left, final int top, final int right, final int bottom, final boolean visible) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                CameraPreviewSurface preview = mCameraPreviewViews.get(videoId);
                if (preview == null) {
                    if (visible) {
                        attachCameraNativePreview(videoId, left, top, right, bottom);
                    }
                    return;
                }
                FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                    Math.max(1, right - left),
                    Math.max(1, bottom - top)
                );
                lp.leftMargin = left;
                lp.topMargin = top;
                preview.setLayoutParams(lp);
                preview.setVisibility(visible ? View.VISIBLE : View.INVISIBLE);
            }
        });
    }

    public void detachCameraNativePreview(final long videoId) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                CameraPreviewSurface preview = mCameraPreviewViews.remove(videoId);
                if (preview != null && mCameraPreviewOverlay != null) {
                    mCameraPreviewOverlay.removeView(preview);
                }
            }
        });
    }

    private static boolean codecLooksSoftware(MediaCodecInfo info) {
        if (Build.VERSION.SDK_INT >= 29) {
            if (info.isSoftwareOnly()) return true;
            if (info.isHardwareAccelerated()) return false;
        }
        String name = info.getName().toLowerCase();
        return name.startsWith("omx.google.") || name.startsWith("c2.android.") || name.contains("sw");
    }

    private static boolean codecLooksHardware(MediaCodecInfo info) {
        if (Build.VERSION.SDK_INT >= 29) {
            if (info.isHardwareAccelerated()) return true;
            if (info.isSoftwareOnly()) return false;
        }
        return !codecLooksSoftware(info);
    }

    public int[] queryH264CodecSupport() {
        boolean encHw = false;
        boolean encSw = false;
        boolean decHw = false;
        boolean decSw = false;
        int maxWidth = 0;
        int maxHeight = 0;
        int maxFps = 0;
        int maxBitrate = 0;
        int widthAlign = 2;
        int heightAlign = 2;

        try {
            MediaCodecList list = new MediaCodecList(MediaCodecList.ALL_CODECS);
            for (MediaCodecInfo info : list.getCodecInfos()) {
                String[] types = info.getSupportedTypes();
                boolean supportsAvc = false;
                for (String t : types) {
                    if ("video/avc".equalsIgnoreCase(t)) {
                        supportsAvc = true;
                        break;
                    }
                }
                if (!supportsAvc) {
                    continue;
                }

                boolean hw = codecLooksHardware(info);
                boolean sw = codecLooksSoftware(info);

                boolean probeOk = false;
                MediaCodec codec = null;
                try {
                    codec = MediaCodec.createByCodecName(info.getName());
                    probeOk = codec != null;
                } catch (Throwable ignored) {
                    probeOk = false;
                } finally {
                    if (codec != null) {
                        try { codec.release(); } catch (Throwable ignored) {}
                    }
                }
                if (!probeOk) {
                    continue;
                }

                try {
                    MediaCodecInfo.CodecCapabilities caps = info.getCapabilitiesForType("video/avc");
                    if (caps != null && caps.getVideoCapabilities() != null) {
                        MediaCodecInfo.VideoCapabilities vc = caps.getVideoCapabilities();
                        maxWidth = Math.max(maxWidth, vc.getSupportedWidths().getUpper().intValue());
                        maxHeight = Math.max(maxHeight, vc.getSupportedHeights().getUpper().intValue());
                        maxBitrate = Math.max(maxBitrate, vc.getBitrateRange().getUpper().intValue());
                        maxFps = Math.max(maxFps, vc.getSupportedFrameRates().getUpper().intValue());
                        widthAlign = Math.max(widthAlign, vc.getWidthAlignment());
                        heightAlign = Math.max(heightAlign, vc.getHeightAlignment());
                    }
                } catch (Throwable ignored) {}

                if (info.isEncoder()) {
                    if (hw) encHw = true;
                    if (sw) encSw = true;
                } else {
                    if (hw) decHw = true;
                    if (sw) decSw = true;
                }
            }
        } catch (Throwable ignored) {}

        return new int[] {
            encHw ? 1 : 0,
            encSw ? 1 : 0,
            decHw ? 1 : 0,
            decSw ? 1 : 0,
            maxWidth,
            maxHeight,
            maxFps,
            maxBitrate,
            widthAlign,
            heightAlign,
        };
    }

    public void prepareVideoPlayback(long videoId, Object source, int externalTextureHandle, boolean autoplay, boolean shouldLoop) {
        VideoPlayer VideoPlayer = new VideoPlayer(this, videoId);
        VideoPlayer.setSource(source);
        VideoPlayer.setExternalTextureHandle(externalTextureHandle);
        VideoPlayer.setAutoplay(autoplay);
        VideoPlayer.setShouldLoop(shouldLoop);
        VideoPlayerRunnable runnable = new VideoPlayerRunnable(VideoPlayer);

        mVideoPlayerRunnables.put(videoId, runnable);
        mVideoPlaybackHandler.post(runnable);
    }

    public void beginVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.beginPlayback();
        }
    }

    public void pauseVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.pausePlayback();
        }
    }

    public void resumeVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.resumePlayback();
        }
    }

    public void muteVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.mute();
        }
    }

    public void unmuteVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.unmute();
        }
    }

    public void seekVideoPlayback(long videoId, long positionMs) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.seekToPosition(positionMs);
        }
    }

    public long getVideoPlaybackPosition(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            return runnable.getCurrentPositionMs();
        }
        return 0;
    }

    public void cleanupVideoPlaybackResources(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.remove(videoId);
        if(runnable != null) {
            runnable.cleanupVideoPlaybackResources();
            runnable = null;
        }
        detachCameraNativePreview(videoId);
    }
    
                
    public boolean isEmulator() {
        // hints that the app is running on emulator
        return Build.MODEL.startsWith("sdk")
            || "google_sdk".equals(Build.MODEL)
            || Build.MODEL.contains("Emulator")
            || Build.MODEL.contains("Android SDK")
            || Build.MODEL.toLowerCase().contains("droid4x")
            || Build.FINGERPRINT.startsWith("generic")
            || Build.PRODUCT == "sdk"
            || Build.PRODUCT == "google_sdk"
            || (Build.BRAND.startsWith("generic") && Build.DEVICE.startsWith("generic"));
    }

    private String getKernelVersion() {
        try {
            Process process = Runtime.getRuntime().exec("uname -r");
            BufferedReader reader = new BufferedReader(new InputStreamReader(process.getInputStream()));
            StringBuilder stringBuilder = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                stringBuilder.append(line);
            }
            return stringBuilder.toString();
        } catch (IOException e) {
            return "Unknown";
        }
    }
    
    

    @SuppressWarnings("deprecation")
    public float getDeviceRefreshRate() {
        float refreshRate = 60.0f;  // Default to a common refresh rate

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // Use getDisplay() API on Android 11 and above
            Display display = getDisplay();
            if (display != null) {
                refreshRate = display.getRefreshRate();
            }
        } else {
            // Use the old method for Android 10 and below
            WindowManager windowManager = (WindowManager) getSystemService(Context.WINDOW_SERVICE);
            if (windowManager != null) {
                Display display = windowManager.getDefaultDisplay();
                refreshRate = display.getRefreshRate();
            }
        }

        return refreshRate;
    }
}
