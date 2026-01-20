package dev.makepad.android;

import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.Manifest;
import android.graphics.Color;
import android.graphics.Insets;
import android.graphics.Rect;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
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
import android.view.Menu;
import android.view.MenuItem;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewTreeObserver;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowManager.LayoutParams;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.CursorAnchorInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.ExtractedText;
import android.view.inputmethod.ExtractedTextRequest;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.SpannableStringBuilder;
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

    // Keyboard configuration (set by Rust via configureKeyboard)
    private int mInputMode = 0;         // 0=Text, 1=Ascii, 2=Url, 3=Numeric, 4=Tel, 5=Email, 6=Decimal, 7=Search
    private int mAutocapitalize = 2;    // 0=None, 1=Words, 2=Sentences, 3=All
    private int mAutocorrect = 0;       // 0=Default, 1=Yes, 2=No
    private int mReturnKeyType = 0;     // 0=Default, 1=Go, 2=Search, 3=Send, 5=Done
    private boolean mIsMultiline = true;
    private boolean mIsSecure = false;

    // Inner class for IME support - uses Editable and delegates to BaseInputConnection
    private class MakepadInputConnection extends BaseInputConnection {
        // Batch edit nesting count
        private int mBatchEditNestCount = 0;
        // For getExtractedText monitoring
        private ExtractedTextRequest mExtractedTextRequest = null;
        private int mExtractedTextToken = 0;
        // For cursor updates
        private int mCursorUpdateMode = 0;
        // Track recent states sent to Rust to detect stale echoes
        // Using a small circular buffer to handle rapid IME operations
        private String[] mRecentSentTexts = new String[4];
        private int mRecentSentIndex = 0;
        private String mLastSentTextToRust = "";

        public MakepadInputConnection(View view, boolean fullEditor) {
            super(view, fullEditor);
        }

        // Check if text was recently sent to Rust (circular buffer lookup)
        private boolean wasRecentlySentToRust(String text) {
            for (String sent : mRecentSentTexts) {
                if (text.equals(sent)) return true;
            }
            return false;
        }

        // Record text as sent to Rust
        private void recordSentToRust(String text) {
            mRecentSentTexts[mRecentSentIndex] = text;
            mRecentSentIndex = (mRecentSentIndex + 1) % mRecentSentTexts.length;
            mLastSentTextToRust = text;
        }

        // Clear recent sent buffer (e.g., after applying genuine Rust update)
        private void clearRecentSentBuffer() {
            for (int i = 0; i < mRecentSentTexts.length; i++) {
                mRecentSentTexts[i] = null;
            }
        }

        // Return our shared Editable - this is the key change!
        // BaseInputConnection methods operate on this Editable automatically
        @Override
        public Editable getEditable() {
            return mEditable;
        }

        @Override
        public boolean beginBatchEdit() {
            mBatchEditNestCount++;
            return true;
        }

        @Override
        public boolean endBatchEdit() {
            if (mBatchEditNestCount > 0) {
                mBatchEditNestCount--;
            }
            // Notify Rust when batch edit completes
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }
            return mBatchEditNestCount > 0;
        }

        @Override
        public ExtractedText getExtractedText(ExtractedTextRequest request, int flags) {
            if (request == null) return null;

            // Remember request if monitoring
            if ((flags & InputConnection.GET_EXTRACTED_TEXT_MONITOR) != 0) {
                mExtractedTextRequest = request;
                mExtractedTextToken = request.token;
            }

            ExtractedText et = new ExtractedText();
            et.text = mEditable.toString();
            et.startOffset = 0;
            et.selectionStart = Selection.getSelectionStart(mEditable);
            et.selectionEnd = Selection.getSelectionEnd(mEditable);

            return et;
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            // Let BaseInputConnection handle span management on mEditable
            boolean result = super.setComposingRegion(start, end);
            // Don't notify Rust here - wait for actual text change
            return result;
        }

        @Override
        public boolean requestCursorUpdates(int cursorUpdateMode) {
            mCursorUpdateMode = cursorUpdateMode;

            if ((cursorUpdateMode & InputConnection.CURSOR_UPDATE_IMMEDIATE) != 0) {
                sendCursorUpdate();
            }
            return true;
        }

        private void sendCursorUpdate() {
            if (mCursorUpdateMode == 0) return;

            InputMethodManager imm = (InputMethodManager)
                getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm == null) return;

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                CursorAnchorInfo.Builder builder = new CursorAnchorInfo.Builder();
                int cursorStart = Selection.getSelectionStart(mEditable);
                int cursorEnd = Selection.getSelectionEnd(mEditable);
                builder.setSelectionRange(cursorStart, cursorEnd);
                builder.setMatrix(new android.graphics.Matrix());
                imm.updateCursorAnchorInfo(MakepadSurface.this, builder.build());
            }
        }

        // Notify IME of current cursor and composition state
        private void notifyImeOfSelectionUpdate() {
            InputMethodManager imm = (InputMethodManager)
                getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm == null) return;

            int selStart = Selection.getSelectionStart(mEditable);
            int selEnd = Selection.getSelectionEnd(mEditable);
            int compStart = BaseInputConnection.getComposingSpanStart(mEditable);
            int compEnd = BaseInputConnection.getComposingSpanEnd(mEditable);
            imm.updateSelection(MakepadSurface.this, selStart, selEnd, compStart, compEnd);
        }

        // Notify Rust of current text state
        private void notifyRustOfTextState() {
            String fullText = mEditable.toString();
            int selStart = Selection.getSelectionStart(mEditable);
            int selEnd = Selection.getSelectionEnd(mEditable);
            int compStart = BaseInputConnection.getComposingSpanStart(mEditable);
            int compEnd = BaseInputConnection.getComposingSpanEnd(mEditable);

            // Track what we're sending so we can detect stale echoes from Rust
            recordSentToRust(fullText);

            MakepadNative.onImeTextStateChanged(fullText, selStart, selEnd, compStart, compEnd);
        }

        // Called from Rust when text changes programmatically (not from IME)
        public void updateFromRust(String fullText, int selStart, int selEnd) {
            // Clear any existing composing spans
            BaseInputConnection.removeComposingSpans(mEditable);

            // Update editable content
            mEditable.replace(0, mEditable.length(), fullText);

            // Clamp selection to valid range
            int textLen = mEditable.length();
            selStart = Math.max(0, Math.min(selStart, textLen));
            selEnd = Math.max(selStart, Math.min(selEnd, textLen));
            Selection.setSelection(mEditable, selStart, selEnd);

            // Force IME to re-query state - CRITICAL for keeping IME in sync
            InputMethodManager imm = (InputMethodManager)
                getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null) {
                imm.restartInput(MakepadSurface.this);
            }
        }

        @Override
        public CharSequence getTextBeforeCursor(int n, int flags) {
            // Delegate to super which uses getEditable()
            return super.getTextBeforeCursor(n, flags);
        }

        @Override
        public CharSequence getTextAfterCursor(int n, int flags) {
            // Delegate to super which uses getEditable()
            return super.getTextAfterCursor(n, flags);
        }

        @Override
        public CharSequence getSelectedText(int flags) {
            // Delegate to super which uses getEditable()
            return super.getSelectedText(flags);
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            // Let BaseInputConnection handle the Editable manipulation
            boolean result = super.setComposingText(text, newCursorPosition);

            // Notify IME of state change
            notifyImeOfSelectionUpdate();

            // Notify Rust (unless in batch edit)
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            // Let BaseInputConnection handle the Editable manipulation
            boolean result = super.commitText(text, newCursorPosition);

            // Notify IME of state change
            notifyImeOfSelectionUpdate();

            // Notify Rust (unless in batch edit)
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public boolean finishComposingText() {
            boolean hadComposition = BaseInputConnection.getComposingSpanStart(mEditable) >= 0;

            // Let BaseInputConnection clear the composing spans
            boolean result = super.finishComposingText();

            // Notify IME
            notifyImeOfSelectionUpdate();

            // Notify Rust if there was a composition
            if (hadComposition && mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            // Let BaseInputConnection handle the Editable manipulation
            boolean result = super.deleteSurroundingText(beforeLength, afterLength);

            // Notify IME of state change
            notifyImeOfSelectionUpdate();

            // Notify Rust (unless in batch edit)
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
            // Use code point deletion which properly handles surrogate pairs (emoji, etc.)
            // This is called by sendKeyEvent for backspace/delete to avoid corrupting strings
            boolean result = super.deleteSurroundingTextInCodePoints(beforeLength, afterLength);

            // Notify IME of state change
            notifyImeOfSelectionUpdate();

            // Notify Rust (unless in batch edit)
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public boolean setSelection(int start, int end) {
            // Short-circuit if already at this selection (prevents Samsung keyboard loop)
            // Samsung may respond to imm.updateSelection() by calling setSelection() again
            int currentStart = Selection.getSelectionStart(mEditable);
            int currentEnd = Selection.getSelectionEnd(mEditable);
            if (currentStart == start && currentEnd == end) {
                return true;  // Already there, no notifications needed
            }

            // Let BaseInputConnection handle selection on Editable
            boolean result = super.setSelection(start, end);

            // Notify IME
            notifyImeOfSelectionUpdate();

            // Notify Rust
            if (mBatchEditNestCount == 0) {
                notifyRustOfTextState();
            }

            return result;
        }

        @Override
        public void closeConnection() {
            super.closeConnection();
        }

        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            // Intercept DELETE key events and translate to deleteSurroundingTextInCodePoints()
            // This is needed for Samsung keyboard delete which uses sendKeyEvent() instead of deleteSurroundingText()
            // sendKeyEvent() dispatches to View asynchronously, which causes sync issues with Samsung
            // We use deleteSurroundingTextInCodePoints (API 24+) instead of deleteSurroundingText
            // because emoji characters are surrogate pairs (2 UTF-16 code units) and we need to
            // delete the full code point, not just one code unit which would corrupt the string.
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                int keyCode = event.getKeyCode();

                if (keyCode == KeyEvent.KEYCODE_DEL) {
                    // Translate backspace to delete 1 code point before cursor
                    // deleteSurroundingTextInCodePoints handles surrogate pairs properly
                    return deleteSurroundingTextInCodePoints(1, 0);
                }

                if (keyCode == KeyEvent.KEYCODE_FORWARD_DEL) {
                    // Translate forward delete (deletes 1 code point after cursor)
                    return deleteSurroundingTextInCodePoints(0, 1);
                }

                if (keyCode == KeyEvent.KEYCODE_ENTER) {
                    // Handle Enter key from IME
                    // Some IMEs send sendKeyEvent(ENTER) instead of commitText("\n")
                    if (mIsMultiline) {
                        // For multiline: insert newline via commitText
                        // This ensures proper notification to Rust via ImeTextState
                        return commitText("\n", 1);
                    }
                    // For single-line: block the Enter key event
                    // The action button (Done/Go/etc) is handled via performEditorAction
                    return true;
                }
            }

            // For other keys (e.g., arrows), use default behavior
            return super.sendKeyEvent(event);
        }

        @Override
        public boolean performEditorAction(int actionCode) {
            // Handle editor actions (Done, Go, Search, Send, Next) for single-line inputs
            // These are triggered when user presses the action button on the soft keyboard

            // EditorInfo action codes:
            // IME_ACTION_UNSPECIFIED = 0, IME_ACTION_NONE = 1, IME_ACTION_GO = 2,
            // IME_ACTION_SEARCH = 3, IME_ACTION_SEND = 4, IME_ACTION_NEXT = 5,
            // IME_ACTION_DONE = 6, IME_ACTION_PREVIOUS = 7

            // Notify Rust about the editor action
            // For single-line inputs, this should trigger TextInputAction::Returned
            MakepadNative.onImeEditorAction(actionCode);

            return true;  // We handled the action
        }
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

        // Initialize selection spans in the Editable - CRITICAL for BaseInputConnection to work
        Selection.setSelection(mEditable, 0, 0);
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Log.i("SAPP", "surfaceCreated");
        Surface surface = holder.getSurface();
        //surface.setFrameRate(120f,0);
        MakepadNative.surfaceOnSurfaceCreated(surface);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        Log.i("SAPP", "surfaceDestroyed");
        Surface surface = holder.getSurface();
        MakepadNative.surfaceOnSurfaceDestroyed(surface);
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder,
                               int format,
                               int width,
                               int height) {
        Log.i("SAPP", "surfaceChanged");
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
        WindowInsets insets = this.getRootWindowInsets();
        if (insets == null) {
            return;
        }

        Rect r = new Rect();
        this.getWindowVisibleDisplayFrame(r);
        int screenHeight = this.getRootView().getHeight();
        int visibleHeight = r.height();
        int keyboardHeight = screenHeight - visibleHeight;

        MakepadNative.surfaceOnResizeTextIME(keyboardHeight, insets.isVisible(WindowInsets.Type.ime()));
    }

    // docs says getCharacters are deprecated
    // but somehow on non-latyn input all keyCode and all the relevant fields in the KeyEvent are zeros
    // and only getCharacters has some usefull data
    @SuppressWarnings("deprecation")
    @Override
    public boolean onKey(View v, int keyCode, KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN && keyCode != 0) {
            int metaState = event.getMetaState();
            MakepadNative.surfaceOnKeyDown(keyCode, metaState);
        }

        if (event.getAction() == KeyEvent.ACTION_UP && keyCode != 0) {
            int metaState = event.getMetaState();
            MakepadNative.surfaceOnKeyUp(keyCode, metaState);
        }

        if (event.getAction() == KeyEvent.ACTION_UP || event.getAction() == KeyEvent.ACTION_MULTIPLE) {
            int character = event.getUnicodeChar();
            if (character == 0) {
                String characters = event.getCharacters();
                if (characters != null && characters.length() >= 0) {
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
        // Build inputType based on configuration
        int inputType = InputType.TYPE_CLASS_TEXT;

        // Input mode
        switch (mInputMode) {
            case 1: // Ascii
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
                break;
            case 2: // Url
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI;
                break;
            case 3: // Numeric
                inputType = InputType.TYPE_CLASS_NUMBER;
                break;
            case 4: // Tel
                inputType = InputType.TYPE_CLASS_PHONE;
                break;
            case 5: // Email
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS;
                break;
            case 6: // Decimal
                inputType = InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL | InputType.TYPE_NUMBER_FLAG_SIGNED;
                break;
            case 7: // Search
                inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_WEB_EDIT_TEXT;
                break;
            default: // Text (0)
                inputType = InputType.TYPE_CLASS_TEXT;
                break;
        }

        // Only add text flags if we're in text class mode
        if ((inputType & InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_TEXT) {
            // Autocapitalization
            switch (mAutocapitalize) {
                case 0: // None
                    // No flag needed
                    break;
                case 1: // Words
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_WORDS;
                    break;
                case 2: // Sentences (default)
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_SENTENCES;
                    break;
                case 3: // AllCharacters
                    inputType |= InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS;
                    break;
            }

            // Autocorrect
            switch (mAutocorrect) {
                case 0: // Default - enable auto-correct
                case 1: // Yes
                    inputType |= InputType.TYPE_TEXT_FLAG_AUTO_CORRECT;
                    break;
                case 2: // No
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

        // Build imeOptions
        int imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI;

        // Return key type
        switch (mReturnKeyType) {
            case 1: // Go
                imeOptions |= EditorInfo.IME_ACTION_GO;
                break;
            case 2: // Search
                imeOptions |= EditorInfo.IME_ACTION_SEARCH;
                break;
            case 3: // Send
                imeOptions |= EditorInfo.IME_ACTION_SEND;
                break;
            case 4: // Next
                imeOptions |= EditorInfo.IME_ACTION_NEXT;
                break;
            case 5: // Done
                imeOptions |= EditorInfo.IME_ACTION_DONE;
                break;
            default: // Default (0)
                if (!mIsMultiline) {
                    imeOptions |= EditorInfo.IME_ACTION_DONE;
                }
                break;
        }

        outAttrs.imeOptions = imeOptions;

        // Set initial selection from our Editable
        int selStart = Selection.getSelectionStart(mEditable);
        int selEnd = Selection.getSelectionEnd(mEditable);
        outAttrs.initialSelStart = Math.max(0, selStart);
        outAttrs.initialSelEnd = Math.max(0, selEnd);

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
            InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null) {
                imm.restartInput(this);
            }
        }
    }

    // Called from Rust to update text state (for programmatic changes, not IME input)
    public void updateImeTextState(String fullText, int selStart, int selEnd) {
        String currentText = mEditable.toString();
        boolean textChanged = !currentText.equals(fullText);

        // Check for stale echoes - if Rust sends back text we recently sent to it,
        // it's just echoing old state and should be ignored to prevent rollback
        if (textChanged && mInputConnection != null) {
            if (mInputConnection.wasRecentlySentToRust(fullText)) {
                // Rust is echoing back something we recently sent - it's stale, ignore
                return;
            }
        }

        // Clamp selection
        int textLen = textChanged ? fullText.length() : mEditable.length();
        selStart = Math.max(0, Math.min(selStart, textLen));
        selEnd = Math.max(selStart, Math.min(selEnd, textLen));

        if (textChanged) {
            // Text content changed - update Editable and restart IME
            // This is a genuine Rust-side change (not an echo)
            BaseInputConnection.removeComposingSpans(mEditable);
            mEditable.replace(0, mEditable.length(), fullText);
            Selection.setSelection(mEditable, selStart, selEnd);

            // Clear stale buffer since we're applying Rust's authoritative state
            // Do NOT call recordSentToRust here - that's only for Java->Rust sends
            // (in onImeTextStateChanged). Recording here would corrupt echo detection.
            if (mInputConnection != null) {
                mInputConnection.clearRecentSentBuffer();
            }

            // Only restart input when text content actually changed
            if (mInputConnection != null) {
                InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) {
                    imm.restartInput(this);
                }
            }
        } else {
            // Only selection changed - just update selection, no restart needed
            int currentSelStart = Selection.getSelectionStart(mEditable);
            int currentSelEnd = Selection.getSelectionEnd(mEditable);
            if (currentSelStart != selStart || currentSelEnd != selEnd) {
                Selection.setSelection(mEditable, selStart, selEnd);

                // Notify IME of selection change without restart
                InputMethodManager imm = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) {
                    int compStart = BaseInputConnection.getComposingSpanStart(mEditable);
                    int compEnd = BaseInputConnection.getComposingSpanEnd(mEditable);
                    imm.updateSelection(this, selStart, selEnd, compStart, compEnd);
                }
            }
        }
    }

    public Surface getNativeSurface() {
        return getHolder().getSurface();
    }
}

class ResizingLayout
    extends
        LinearLayout
    implements
        View.OnApplyWindowInsetsListener {

    public ResizingLayout(Context context){
        super(context);
        // When viewing in landscape mode with keyboard shown, there are
        // gaps on both sides so we fill the negative space with black.
        setBackgroundColor(Color.BLACK);
        setOnApplyWindowInsetsListener(this);
    }

    @Override
    public WindowInsets onApplyWindowInsets(View v, WindowInsets insets) {
        Insets imeInsets = insets.getInsets(WindowInsets.Type.ime());
        v.setPadding(0, 0, 0, imeInsets.bottom);
        return insets;
    }
}

public class MakepadActivity
    extends Activity
    implements MidiManager.OnDeviceOpenedListener
{
    //% MAIN_ACTIVITY_BODY

    private MakepadSurface view;
    Handler mHandler;

    // video playback
    Handler mVideoPlaybackHandler;
    HashMap<Long, VideoPlayerRunnable> mVideoPlayerRunnables;

    // networking, make these static because of activity switching
    static Handler mWebSocketsHandler;
    static HashMap<Long, MakepadWebSocket> mActiveWebsockets = new HashMap<>();
    static HashMap<Long, MakepadWebSocketReader> mActiveWebsocketsReaders = new HashMap<>();

    // clipboard actions (ActionMode for copy/paste/cut)
    private ActionMode mActionMode;
    private boolean mHasSelection = false;
    private int[] mSelectionBounds = new int[4]; // left, top, right, bottom
    private int mKeyboardShift = 0; // keyboard shift amount from Rust

    static {
        System.loadLibrary("makepad");
    }

    @Override
    public void onCreate(Bundle savedInstanceState) {
        
        HandlerThread webSocketsThreadHandler = new HandlerThread("WebSocketsThread");
        webSocketsThreadHandler.start();
        mWebSocketsHandler = new Handler(webSocketsThreadHandler.getLooper());
        
        super.onCreate(savedInstanceState);
        
        this.requestWindowFeature(Window.FEATURE_NO_TITLE);

        view = new MakepadSurface(this);
        // Put it inside a parent layout which can resize it using padding
        ResizingLayout layout = new ResizingLayout(this);
        layout.addView(view);
        setContentView(layout);

        MakepadNative.activityOnCreate(this);

        HandlerThread decoderThreadHandler = new HandlerThread("VideoPlayerThread");
        decoderThreadHandler.start(); // TODO: only start this if its needed.
        mVideoPlaybackHandler = new Handler(decoderThreadHandler.getLooper());
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
        MakepadNative.activityOnStart();
    }

    @Override
    protected void onResume() {
        super.onResume();
        MakepadNative.activityOnResume();

        //% MAIN_ACTIVITY_ON_RESUME
    }
    @Override
    protected void onPause() {
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
        super.onDestroy();
        MakepadNative.activityOnDestroy();
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
                    View decorView = getWindow().getDecorView();

                    if (fullscreen) {
                        getWindow().setFlags(LayoutParams.FLAG_LAYOUT_NO_LIMITS, LayoutParams.FLAG_LAYOUT_NO_LIMITS);
                        getWindow().getAttributes().layoutInDisplayCutoutMode = LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
                        if (Build.VERSION.SDK_INT >= 30) {
                            getWindow().setDecorFitsSystemWindows(false);
                        } else {
                            int uiOptions = View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY;
                            decorView.setSystemUiVisibility(uiOptions);
                        }
                    }
                    else {
                        if (Build.VERSION.SDK_INT >= 30) {
                            getWindow().setDecorFitsSystemWindows(true);
                        } else {
                          decorView.setSystemUiVisibility(0);
                        }

                    }
                }
            });
    }
    
    public void switchActivityClass(Class c){
        Intent intent = new Intent(getApplicationContext(), c);
        startActivity(intent);
        finish();
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
                    InputMethodManager imm = (InputMethodManager)getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.showSoftInput(view, 0);
                } else {
                    InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.hideSoftInputFromWindow(view.getWindowToken(),0);
                }
            }
        });
    }

    // Update IME text state for programmatic changes - called from Rust
    // Note: This should only be called for programmatic text changes (e.g., clear button),
    // NOT during normal IME input (which flows Java→Rust via onImeTextStateChanged)
    public void updateImeTextState(final String fullText, final int selStart, final int selEnd) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (view != null) {
                    view.updateImeTextState(fullText, selStart, selEnd);
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
        // Enable/disable menu items based on state
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

            // After select all, re-show the menu with Copy/Cut options
            // Post delayed to give Rust time to update selection
            final ActionMode currentMode = mode;
            new Handler(Looper.getMainLooper()).postDelayed(new Runnable() {
                @Override
                public void run() {
                    if (currentMode != null) {
                        mHasSelection = true;
                        currentMode.invalidate();
                    }
                }
            }, 50); // 50ms delay

            return true; // Don't finish mode - let it update
        }
        return false;
    }

    private void onDestroyActionModeInternal(ActionMode mode) {
        mActionMode = null;
        mHasSelection = false;
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
                        mm.openBluetoothDevice(device, this, new Handler(Looper.getMainLooper()));
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
                        mm.openDevice(info, this, new Handler(Looper.getMainLooper()));
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

    public void cleanupVideoPlaybackResources(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.remove(videoId);
        if(runnable != null) {
            runnable.cleanupVideoPlaybackResources();
            runnable = null;
        }
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
