package dev.makepad.android;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Bundle;
import android.os.Build;
import android.text.Editable;
import android.text.Selection;
import android.text.TextUtils;
import android.view.KeyEvent;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.CompletionInfo;
import android.view.inputmethod.CorrectionInfo;
import android.view.inputmethod.CursorAnchorInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.ExtractedText;
import android.view.inputmethod.ExtractedTextRequest;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.view.inputmethod.SurroundingText;
import android.view.inputmethod.TextAttribute;
import android.view.inputmethod.TextSnapshot;

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
    private boolean mPendingStateNotification = false;
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

    private int clampIndex(int index, int length) {
        if (index < 0) return length;
        return Math.max(0, Math.min(index, length));
    }

    private int selectionStart(Editable editable) {
        int length = editable.length();
        int start = clampIndex(Selection.getSelectionStart(editable), length);
        int end = clampIndex(Selection.getSelectionEnd(editable), length);
        return Math.min(start, end);
    }

    private int selectionEnd(Editable editable) {
        int length = editable.length();
        int start = clampIndex(Selection.getSelectionStart(editable), length);
        int end = clampIndex(Selection.getSelectionEnd(editable), length);
        return Math.max(start, end);
    }

    private int selectionStartRaw(Editable editable) {
        return clampIndex(Selection.getSelectionStart(editable), editable.length());
    }

    private int selectionEndRaw(Editable editable) {
        return clampIndex(Selection.getSelectionEnd(editable), editable.length());
    }

    private SurroundingText surroundingTextForLengths(int beforeLength, int afterLength) {
        if (beforeLength < 0 || afterLength < 0) {
            throw new IllegalArgumentException("beforeLength and afterLength must be non-negative");
        }

        Editable editable = mSurface.getEditable();
        int textLength = editable.length();
        int selStart = selectionStart(editable);
        int selEnd = selectionEnd(editable);
        int surroundingStart = Math.max(0, selStart - beforeLength);
        int surroundingEnd = Math.min(textLength, selEnd + afterLength);

        surroundingStart = adjustStartForSurrogate(editable, surroundingStart);
        surroundingEnd = adjustEndForSurrogate(editable, surroundingEnd);

        CharSequence text = editable.subSequence(surroundingStart, surroundingEnd);
        return new SurroundingText(
            text,
            selectionStartRaw(editable) - surroundingStart,
            selectionEndRaw(editable) - surroundingStart,
            surroundingStart);
    }

    private void notifyStateChanged() {
        if (mBatchEditNestCount > 0) {
            mPendingStateNotification = true;
            return;
        }
        notifyImeOfSelectionUpdate();
        notifyRustOfTextState();
    }

    private boolean isTextKey(KeyEvent event) {
        return event.getUnicodeChar() != 0
            && !event.isCtrlPressed()
            && !event.isAltPressed();
    }

    private boolean isNavigationKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_DPAD_LEFT
            || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT
            || keyCode == KeyEvent.KEYCODE_DPAD_UP
            || keyCode == KeyEvent.KEYCODE_DPAD_DOWN
            || keyCode == KeyEvent.KEYCODE_MOVE_HOME
            || keyCode == KeyEvent.KEYCODE_MOVE_END
            || keyCode == KeyEvent.KEYCODE_PAGE_UP
            || keyCode == KeyEvent.KEYCODE_PAGE_DOWN;
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
        if (mBatchEditNestCount == 0 && mPendingStateNotification) {
            mPendingStateNotification = false;
            notifyStateChanged();
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
        et.selectionStart = clampIndex(Selection.getSelectionStart(editable), editable.length());
        et.selectionEnd = clampIndex(Selection.getSelectionEnd(editable), editable.length());
        et.partialStartOffset = -1;
        et.partialEndOffset = -1;

        return et;
    }

    @Override
    public boolean setComposingRegion(int start, int end) {
        Editable editable = mSurface.getEditable();
        int textLength = editable.length();
        start = clampIndex(start, textLength);
        end = clampIndex(end, textLength);

        // Let BaseInputConnection handle span management on Editable
        boolean result = super.setComposingRegion(Math.min(start, end), Math.max(start, end));
        // Don't notify Rust here - wait for actual text change
        return result;
    }

    @Override
    public boolean setComposingRegion(int start, int end, TextAttribute textAttribute) {
        return setComposingRegion(start, end);
    }

    @Override
    public boolean requestCursorUpdates(int cursorUpdateMode) {
        mCursorUpdateMode = cursorUpdateMode;

        if ((cursorUpdateMode & InputConnection.CURSOR_UPDATE_IMMEDIATE) != 0) {
            sendCursorUpdate();
        }
        return true;
    }

    @Override
    public boolean requestCursorUpdates(int cursorUpdateMode, int cursorUpdateFilter) {
        return requestCursorUpdates(cursorUpdateMode);
    }

    @Override
    public int getCursorCapsMode(int reqModes) {
        Editable editable = mSurface.getEditable();
        int cursor = clampIndex(Selection.getSelectionEnd(editable), editable.length());
        return TextUtils.getCapsMode(editable, cursor, reqModes);
    }

    private void sendCursorUpdate() {
        if (mCursorUpdateMode == 0) return;

        InputMethodManager imm = (InputMethodManager)
            mSurface.getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm == null) return;

        Editable editable = mSurface.getEditable();

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
            CursorAnchorInfo.Builder builder = new CursorAnchorInfo.Builder();
            int cursorStart = clampIndex(Selection.getSelectionStart(editable), editable.length());
            int cursorEnd = clampIndex(Selection.getSelectionEnd(editable), editable.length());
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
        int textLength = editable.length();
        selStart = clampIndex(selStart, textLength);
        selEnd = clampIndex(selEnd, textLength);
        if (compStart < 0 || compEnd < 0) {
            compStart = -1;
            compEnd = -1;
        } else {
            compStart = clampIndex(compStart, textLength);
            compEnd = clampIndex(compEnd, textLength);
        }
        imm.updateSelection(mSurface, selStart, selEnd, compStart, compEnd);
    }

    // Notify Rust of current text state
    private void notifyRustOfTextState() {
        Editable editable = mSurface.getEditable();
        String fullText = editable.toString();
        int textLength = editable.length();
        int selStart = clampIndex(Selection.getSelectionStart(editable), textLength);
        int selEnd = clampIndex(Selection.getSelectionEnd(editable), textLength);
        int compStart = BaseInputConnection.getComposingSpanStart(editable);
        int compEnd = BaseInputConnection.getComposingSpanEnd(editable);
        if (compStart < 0 || compEnd < 0) {
            compStart = -1;
            compEnd = -1;
        } else {
            compStart = clampIndex(compStart, textLength);
            compEnd = clampIndex(compEnd, textLength);
        }

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
    public SurroundingText getSurroundingText(int beforeLength, int afterLength, int flags) {
        return surroundingTextForLengths(beforeLength, afterLength);
    }

    @Override
    public TextSnapshot takeSnapshot() {
        Editable editable = mSurface.getEditable();
        SurroundingText surroundingText = new SurroundingText(
            editable.toString(),
            selectionStartRaw(editable),
            selectionEndRaw(editable),
            0);
        int compStart = BaseInputConnection.getComposingSpanStart(editable);
        int compEnd = BaseInputConnection.getComposingSpanEnd(editable);
        int textLength = editable.length();
        if (compStart < 0 || compEnd < 0) {
            compStart = -1;
            compEnd = -1;
        } else {
            compStart = clampIndex(compStart, textLength);
            compEnd = clampIndex(compEnd, textLength);
        }
        return new TextSnapshot(
            surroundingText,
            compStart,
            compEnd,
            getCursorCapsMode(TextUtils.CAP_MODE_CHARACTERS
                | TextUtils.CAP_MODE_WORDS
                | TextUtils.CAP_MODE_SENTENCES));
    }

    @Override
    public boolean setComposingText(CharSequence text, int newCursorPosition) {
        CharSequence filtered = filterInput(text);
        if (filtered == null) {
            filtered = "";
        }
        if (filtered.length() == 0 && text != null && text.length() > 0) {
            return true;
        }

        // Let BaseInputConnection handle the Editable manipulation
        boolean result = super.setComposingText(filtered, newCursorPosition);

        notifyStateChanged();

        return result;
    }

    @Override
    public boolean setComposingText(
            CharSequence text, int newCursorPosition, TextAttribute textAttribute) {
        return setComposingText(text, newCursorPosition);
    }

    @Override
    public boolean commitText(CharSequence text, int newCursorPosition) {
        // Filter input based on input mode (e.g., prevent emojis in numeric fields)
        CharSequence filtered = filterInput(text);
        if (filtered == null) {
            filtered = "";
        }
        if (filtered.length() == 0 && text != null && text.length() > 0) {
            // All characters were filtered out - consume but don't insert
            return true;
        }

        // Let BaseInputConnection handle the Editable manipulation
        boolean result = super.commitText(filtered, newCursorPosition);

        notifyStateChanged();

        return result;
    }

    @Override
    public boolean commitText(
            CharSequence text, int newCursorPosition, TextAttribute textAttribute) {
        return commitText(text, newCursorPosition);
    }

    // API 34 adds this to InputConnection. Keep the method present even when
    // compiling against API 33, which is Makepad's bundled SDK level.
    public boolean replaceText(
            int start,
            int end,
            CharSequence text,
            int newCursorPosition,
            TextAttribute textAttribute) {
        Editable editable = mSurface.getEditable();
        int textLength = editable.length();
        start = clampIndex(start, textLength);
        end = clampIndex(end, textLength);
        beginBatchEdit();
        boolean result;
        try {
            finishComposingText();
            setSelection(Math.min(start, end), Math.max(start, end));
            result = commitText(text, newCursorPosition);
        } finally {
            endBatchEdit();
        }
        return result;
    }

    @Override
    public boolean finishComposingText() {
        Editable editable = mSurface.getEditable();
        boolean hadComposition = BaseInputConnection.getComposingSpanStart(editable) >= 0;

        // Let BaseInputConnection clear the composing spans
        boolean result = super.finishComposingText();

        if (hadComposition) {
            notifyStateChanged();
        } else {
            notifyImeOfSelectionUpdate();
        }

        return result;
    }

    @Override
    public boolean deleteSurroundingText(int beforeLength, int afterLength) {
        Editable editable = mSurface.getEditable();
        int start = selectionStart(editable);
        int end = selectionEnd(editable);

        if (start == end) {
            int deleteStart = adjustStartForSurrogate(
                editable, Math.max(0, start - Math.max(0, beforeLength)));
            int deleteEnd = adjustEndForSurrogate(
                editable, Math.min(editable.length(), end + Math.max(0, afterLength)));
            if (deleteStart < deleteEnd) {
                editable.delete(deleteStart, deleteEnd);
                Selection.setSelection(editable, deleteStart, deleteStart);
                BaseInputConnection.removeComposingSpans(editable);
            }
        } else {
            // The InputConnection contract deletes surrounding text outside the
            // selection, not the selection itself.
            int beforeStart = adjustStartForSurrogate(
                editable, Math.max(0, start - Math.max(0, beforeLength)));
            int afterEnd = adjustEndForSurrogate(
                editable, Math.min(editable.length(), end + Math.max(0, afterLength)));

            if (end < afterEnd) {
                editable.delete(end, afterEnd);
            }
            if (beforeStart < start) {
                editable.delete(beforeStart, start);
                int newEnd = beforeStart + (end - start);
                Selection.setSelection(editable, beforeStart, newEnd);
            } else {
                Selection.setSelection(editable, start, end);
            }
            if (beforeStart < start || end < afterEnd) {
                BaseInputConnection.removeComposingSpans(editable);
            }
        }

        notifyStateChanged();

        return true;
    }

    @Override
    public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
        Editable editable = mSurface.getEditable();
        int start = selectionStart(editable);
        int end = selectionEnd(editable);

        if (start == end) {
            int deleteStart = moveByCodePoints(editable, start, -Math.max(0, beforeLength));
            int deleteEnd = moveByCodePoints(editable, end, Math.max(0, afterLength));
            if (deleteStart < deleteEnd) {
                editable.delete(deleteStart, deleteEnd);
                Selection.setSelection(editable, deleteStart, deleteStart);
                BaseInputConnection.removeComposingSpans(editable);
            }
        } else {
            int beforeStart = moveByCodePoints(editable, start, -Math.max(0, beforeLength));
            int afterEnd = moveByCodePoints(editable, end, Math.max(0, afterLength));

            if (end < afterEnd) {
                editable.delete(end, afterEnd);
            }
            if (beforeStart < start) {
                editable.delete(beforeStart, start);
                int newEnd = beforeStart + (end - start);
                Selection.setSelection(editable, beforeStart, newEnd);
            } else {
                Selection.setSelection(editable, start, end);
            }
            if (beforeStart < start || end < afterEnd) {
                BaseInputConnection.removeComposingSpans(editable);
            }
        }

        notifyStateChanged();

        return true;
    }

    private int adjustStartForSurrogate(CharSequence text, int index) {
        if (index > 0
                && index < text.length()
                && Character.isLowSurrogate(text.charAt(index))
                && Character.isHighSurrogate(text.charAt(index - 1))) {
            return index - 1;
        }
        return index;
    }

    private int adjustEndForSurrogate(CharSequence text, int index) {
        if (index > 0
                && index < text.length()
                && Character.isLowSurrogate(text.charAt(index))
                && Character.isHighSurrogate(text.charAt(index - 1))) {
            return index + 1;
        }
        return index;
    }

    private int moveByCodePoints(CharSequence text, int index, int codePointDelta) {
        int length = text.length();
        index = Math.max(0, Math.min(index, length));

        if (codePointDelta > 0) {
            for (int i = 0; i < codePointDelta && index < length; i++) {
                int codePoint = Character.codePointAt(text, index);
                index += Character.charCount(codePoint);
            }
        } else {
            for (int i = 0; i < -codePointDelta && index > 0; i++) {
                int codePoint = Character.codePointBefore(text, index);
                index -= Character.charCount(codePoint);
            }
        }

        return index;
    }

    @Override
    public boolean setSelection(int start, int end) {
        Editable editable = mSurface.getEditable();
        int textLength = editable.length();
        start = clampIndex(start, textLength);
        end = clampIndex(end, textLength);

        // Short-circuit if already at this selection (prevents Samsung keyboard loop)
        // Samsung may respond to imm.updateSelection() by calling setSelection() again
        int currentStart = Selection.getSelectionStart(editable);
        int currentEnd = Selection.getSelectionEnd(editable);
        int compStart = BaseInputConnection.getComposingSpanStart(editable);
        int compEnd = BaseInputConnection.getComposingSpanEnd(editable);
        boolean clearsComposition = false;
        if (compStart >= 0 && compEnd >= 0) {
            int compMin = Math.min(compStart, compEnd);
            int compMax = Math.max(compStart, compEnd);
            int selMin = Math.min(start, end);
            int selMax = Math.max(start, end);
            clearsComposition = selMin < compMin || selMax > compMax;
        }
        if (currentStart == start && currentEnd == end && !clearsComposition) {
            return true;  // Already there, no notifications needed
        }

        // Let BaseInputConnection handle selection on Editable
        boolean result = super.setSelection(start, end);
        if (clearsComposition) {
            BaseInputConnection.removeComposingSpans(editable);
        }

        notifyStateChanged();

        return result;
    }

    @Override
    public boolean performContextMenuAction(int id) {
        Editable editable = mSurface.getEditable();
        int start = selectionStart(editable);
        int end = selectionEnd(editable);

        if (id == android.R.id.selectAll) {
            Selection.setSelection(editable, 0, editable.length());
            notifyStateChanged();
            return true;
        }

        if (id == android.R.id.copy || id == android.R.id.cut) {
            if (start < end) {
                ClipboardManager clipboard = (ClipboardManager)
                    mSurface.getContext().getSystemService(Context.CLIPBOARD_SERVICE);
                if (clipboard != null) {
                    CharSequence selected = editable.subSequence(start, end);
                    clipboard.setPrimaryClip(ClipData.newPlainText("text", selected));
                }
            }

            if (id == android.R.id.cut) {
                if (start < end) {
                    return commitText("", 1);
                }
                return true;
            }

            return true;
        }

        if (id == android.R.id.paste) {
            ClipboardManager clipboard = (ClipboardManager)
                mSurface.getContext().getSystemService(Context.CLIPBOARD_SERVICE);
            if (clipboard != null && clipboard.hasPrimaryClip() && clipboard.getPrimaryClip() != null) {
                CharSequence text = clipboard.getPrimaryClip().getItemAt(0)
                    .coerceToText(mSurface.getContext());
                if (text != null) {
                    return commitText(text, 1);
                }
            }
            return true;
        }

        return super.performContextMenuAction(id);
    }

    @Override
    public boolean commitCompletion(CompletionInfo text) {
        if (text == null) {
            return true;
        }
        CharSequence completion = text.getText();
        if (completion == null) {
            return true;
        }
        return commitText(completion, 1);
    }

    @Override
    public boolean commitCorrection(CorrectionInfo correctionInfo) {
        // Android uses this as an editor notification that a correction happened.
        // Keyboards that need text changes also send commitText/setComposingText,
        // so acknowledging here avoids double-applying the correction.
        return true;
    }

    @Override
    public boolean performPrivateCommand(String action, Bundle data) {
        // Private commands are only meaningful when an editor and IME have an
        // explicit app-specific protocol. Makepad does not advertise one, but
        // acknowledging keeps IMEs that send benign probes from treating the
        // connection as broken.
        return true;
    }

    @Override
    public boolean performSpellCheck() {
        // The editor can ignore this request; text mutation remains driven by
        // subsequent commitText/setComposingText/replaceText calls.
        return true;
    }

    @Override
    public boolean setImeConsumesInput(boolean imeConsumesInput) {
        return true;
    }

    @Override
    public boolean clearMetaKeyStates(int states) {
        return true;
    }

    @Override
    public boolean reportFullscreenMode(boolean enabled) {
        return true;
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
        int action = event.getAction();
        int keyCode = event.getKeyCode();
        if (isNavigationKey(keyCode)) {
            int metaState = event.getMetaState();
            if (action == KeyEvent.ACTION_DOWN) {
                MakepadNative.surfaceOnKeyDown(keyCode, metaState, event.getRepeatCount() > 0);
            } else if (action == KeyEvent.ACTION_UP) {
                MakepadNative.surfaceOnKeyUp(keyCode, metaState);
            }
            return true;
        }

        boolean handledTextKey = keyCode == KeyEvent.KEYCODE_DEL
            || keyCode == KeyEvent.KEYCODE_FORWARD_DEL
            || keyCode == KeyEvent.KEYCODE_ENTER
            || isTextKey(event);

        if (action == KeyEvent.ACTION_UP && handledTextKey) {
            return true;
        }

        if (action == KeyEvent.ACTION_DOWN) {
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

            if (isTextKey(event)) {
                int unicode = event.getUnicodeChar();
                return commitText(new String(Character.toChars(unicode)), 1);
            }
        }

        // For other keys, use default behavior.
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

        if (!mSurface.isMultiline() && actionCode <= EditorInfo.IME_ACTION_NONE) {
            // Some keyboards report the visually configured "Done" key as an
            // unspecified action for custom editors. Treat it as Done for
            // single-line fields so accepting the action never inserts text.
            actionCode = EditorInfo.IME_ACTION_DONE;
        }

        // Notify Rust about the editor action
        // For single-line inputs, this should trigger TextInputAction::Returned
        MakepadNative.onImeEditorAction(actionCode);

        return true;
    }
}
