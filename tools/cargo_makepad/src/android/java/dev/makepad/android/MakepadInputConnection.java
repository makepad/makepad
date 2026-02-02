package dev.makepad.android;

import android.content.Context;
import android.os.Build;
import android.text.Selection;
import android.view.KeyEvent;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.CursorAnchorInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.ExtractedText;
import android.view.inputmethod.ExtractedTextRequest;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.text.Editable;

/**
 * IME InputConnection implementation for Makepad.
 *
 * This class handles IME (Input Method Editor) communication for text input.
 * It uses BaseInputConnection with an Editable buffer owned by MakepadSurface.
 */
public class MakepadInputConnection extends BaseInputConnection {
    // Reference to surface for accessing shared state
    private MakepadSurface mSurface;

    // Batch edit nesting count
    private int mBatchEditNestCount = 0;
    // For getExtractedText monitoring
    ExtractedTextRequest mExtractedTextRequest = null;
    int mExtractedTextToken = 0;
    // For cursor updates
    private int mCursorUpdateMode = 0;
    // Echo prevention: Track last text sent to Rust to detect stale echoes
    private String mLastSentText = null;

    public MakepadInputConnection(MakepadSurface surface, boolean fullEditor) {
        super(surface, fullEditor);
        mSurface = surface;
    }

    // Check if text was recently sent to Rust
    boolean wasRecentlySentToRust(String text) {
        return text.equals(mLastSentText);
    }

    // Record text as sent to Rust
    private void recordSentToRust(String text) {
        mLastSentText = text;
    }

    // Clear sent buffer (e.g., after applying genuine Rust update)
    void clearRecentSentBuffer() {
        mLastSentText = null;
    }

    // Filter input based on input mode to prevent invalid characters (e.g., emojis in numeric fields)
    private CharSequence filterInput(CharSequence text) {
        if (text == null || text.length() == 0) return text;

        int inputMode = mSurface.getInputMode();
        Editable editable = mSurface.getEditable();

        switch (inputMode) {
            case MakepadSurface.INPUT_MODE_ASCII:
                StringBuilder ascii = new StringBuilder();
                for (int i = 0; i < text.length(); i++) {
                    char c = text.charAt(i);
                    if (c < 128) ascii.append(c);
                }
                return ascii;
            case MakepadSurface.INPUT_MODE_NUMERIC:
                StringBuilder numeric = new StringBuilder();
                for (int i = 0; i < text.length(); i++) {
                    char c = text.charAt(i);
                    if (Character.isDigit(c)) numeric.append(c);
                }
                return numeric;
            case MakepadSurface.INPUT_MODE_DECIMAL:
                StringBuilder decimal = new StringBuilder();
                boolean hasDot = editable.toString().contains(".");
                for (int i = 0; i < text.length(); i++) {
                    char c = text.charAt(i);
                    if (Character.isDigit(c) || c == '-' || c == '+') {
                        decimal.append(c);
                    } else if (c == '.' && !hasDot) {
                        decimal.append(c);
                        hasDot = true;
                    }
                }
                return decimal;
            case MakepadSurface.INPUT_MODE_TEL:
                StringBuilder tel = new StringBuilder();
                for (int i = 0; i < text.length(); i++) {
                    char c = text.charAt(i);
                    if (Character.isDigit(c) || c == '+' || c == '-' || c == ' '
                        || c == '(' || c == ')' || c == '*' || c == '#') {
                        tel.append(c);
                    }
                }
                return tel;
            default: // TEXT, URL, EMAIL, SEARCH - allow all
                return text;
        }
    }

    // Return the shared Editable from surface - this is the key change!
    // BaseInputConnection methods operate on this Editable automatically
    @Override
    public Editable getEditable() {
        return mSurface.getEditable();
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

        Editable editable = mSurface.getEditable();

        // Remember request if monitoring
        if ((flags & InputConnection.GET_EXTRACTED_TEXT_MONITOR) != 0) {
            mExtractedTextRequest = request;
            mExtractedTextToken = request.token;
        }

        ExtractedText et = new ExtractedText();
        et.text = editable.toString();
        et.startOffset = 0;
        et.selectionStart = Selection.getSelectionStart(editable);
        et.selectionEnd = Selection.getSelectionEnd(editable);

        return et;
    }

    @Override
    public boolean setComposingRegion(int start, int end) {
        // Let BaseInputConnection handle span management on Editable
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
            mSurface.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm == null) return;

        Editable editable = mSurface.getEditable();

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
            CursorAnchorInfo.Builder builder = new CursorAnchorInfo.Builder();
            int cursorStart = Selection.getSelectionStart(editable);
            int cursorEnd = Selection.getSelectionEnd(editable);
            builder.setSelectionRange(cursorStart, cursorEnd);
            builder.setMatrix(new android.graphics.Matrix());
            imm.updateCursorAnchorInfo(mSurface, builder.build());
        }
    }

    // Notify IME of current cursor and composition state
    void notifyImeOfSelectionUpdate() {
        InputMethodManager imm = (InputMethodManager)
            mSurface.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm == null) return;

        Editable editable = mSurface.getEditable();
        int selStart = Selection.getSelectionStart(editable);
        int selEnd = Selection.getSelectionEnd(editable);
        int compStart = BaseInputConnection.getComposingSpanStart(editable);
        int compEnd = BaseInputConnection.getComposingSpanEnd(editable);
        imm.updateSelection(mSurface, selStart, selEnd, compStart, compEnd);
    }

    // Notify Rust of current text state
    private void notifyRustOfTextState() {
        Editable editable = mSurface.getEditable();
        String fullText = editable.toString();
        int selStart = Selection.getSelectionStart(editable);
        int selEnd = Selection.getSelectionEnd(editable);
        int compStart = BaseInputConnection.getComposingSpanStart(editable);
        int compEnd = BaseInputConnection.getComposingSpanEnd(editable);

        // ECHO PREVENTION: Record text before sending to Rust so we can detect
        // if Rust echoes it back via updateImeTextState(). See architecture comment
        // at class definition for full explanation.
        recordSentToRust(fullText);

        MakepadNative.onImeTextStateChanged(fullText, selStart, selEnd, compStart, compEnd);
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
        // Filter input based on input mode (e.g., prevent emojis in numeric fields)
        CharSequence filtered = filterInput(text);
        if (filtered.length() == 0 && text.length() > 0) {
            // All characters were filtered out - consume but don't insert
            return true;
        }

        // Let BaseInputConnection handle the Editable manipulation
        boolean result = super.commitText(filtered, newCursorPosition);

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
        Editable editable = mSurface.getEditable();
        boolean hadComposition = BaseInputConnection.getComposingSpanStart(editable) >= 0;

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
        Editable editable = mSurface.getEditable();

        // Short-circuit if already at this selection (prevents Samsung keyboard loop)
        // Samsung may respond to imm.updateSelection() by calling setSelection() again
        int currentStart = Selection.getSelectionStart(editable);
        int currentEnd = Selection.getSelectionEnd(editable);
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
            Editable editable = mSurface.getEditable();

            if (keyCode == KeyEvent.KEYCODE_DEL) {
                // Check if there's a selection to delete
                int selStart = Selection.getSelectionStart(editable);
                int selEnd = Selection.getSelectionEnd(editable);
                if (selStart != selEnd) {
                    // Selection exists - delete it by replacing with empty text
                    return commitText("", 1);
                }
                // No selection - delete one code point before cursor
                // deleteSurroundingTextInCodePoints handles surrogate pairs properly
                return deleteSurroundingTextInCodePoints(1, 0);
            }

            if (keyCode == KeyEvent.KEYCODE_FORWARD_DEL) {
                // Check if there's a selection to delete
                int selStart = Selection.getSelectionStart(editable);
                int selEnd = Selection.getSelectionEnd(editable);
                if (selStart != selEnd) {
                    // Selection exists - delete it by replacing with empty text
                    return commitText("", 1);
                }
                // No selection - delete one code point after cursor
                return deleteSurroundingTextInCodePoints(0, 1);
            }

            if (keyCode == KeyEvent.KEYCODE_ENTER) {
                // Handle Enter key from IME
                // Some IMEs send sendKeyEvent(ENTER) instead of commitText("\n")
                if (mSurface.isMultiline()) {
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
        // Handle editor actions (Done, Go, Search, Send, Next)
        // These are triggered when user presses the action button on the soft keyboard

        // EditorInfo action codes:
        // IME_ACTION_UNSPECIFIED = 0, IME_ACTION_NONE = 1, IME_ACTION_GO = 2,
        // IME_ACTION_SEARCH = 3, IME_ACTION_SEND = 4, IME_ACTION_NEXT = 5,
        // IME_ACTION_DONE = 6, IME_ACTION_PREVIOUS = 7

        if (mSurface.isMultiline() && actionCode <= EditorInfo.IME_ACTION_NONE) {
            // For multiline with unspecified/none action, insert a newline.
            // Some IMEs (e.g. SwiftKey) call performEditorAction(IME_ACTION_UNSPECIFIED)
            // instead of sendKeyEvent(KEYCODE_ENTER) or commitText("\n").
            return commitText("\n", 1);
        }

        // Notify Rust about the editor action
        // For single-line inputs, this should trigger TextInputAction::Returned
        MakepadNative.onImeEditorAction(actionCode);

        return true;
    }
}
