package dev.makepad.android;

import android.content.Context;
import android.graphics.Rect;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewParent;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityManager;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityNodeProvider;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;

/** A virtual view of the HAL semantic mirror. Every node read is one compact native record. */
public final class MakepadA11yProvider extends AccessibilityNodeProvider {
    private static final int NONE = Integer.MIN_VALUE;
    private static final int HEADER = 28;
    private static final int STRINGS = 6;
    private final View host;
    private final AccessibilityManager manager;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final AccessibilityManager.AccessibilityStateChangeListener stateListener;
    private final AccessibilityManager.TouchExplorationStateChangeListener explorationListener;
    private int accessibilityFocus = NONE;
    private int hovered = NONE;
    private long lastRequest;
    private boolean attached;

    MakepadA11yProvider(View host) {
        this.host = host;
        manager = (AccessibilityManager) host.getContext().getSystemService(Context.ACCESSIBILITY_SERVICE);
        stateListener = enabled -> updateState();
        explorationListener = enabled -> updateState();
        host.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_YES);
    }

    void attach() {
        if (attached) return;
        attached = true;
        if (manager != null) {
            manager.addAccessibilityStateChangeListener(stateListener);
            manager.addTouchExplorationStateChangeListener(explorationListener);
        }
        updateState();
    }

    void detach() {
        if (!attached) return;
        attached = false;
        if (manager != null) {
            manager.removeAccessibilityStateChangeListener(stateListener);
            manager.removeTouchExplorationStateChangeListener(explorationListener);
        }
        accessibilityFocus = NONE;
        hovered = NONE;
        MakepadNative.a11yState(false, false);
    }

    void updateState() {
        boolean enabled = attached && manager != null && manager.isEnabled();
        MakepadNative.a11yState(enabled, enabled && manager.isTouchExplorationEnabled());
        if (!enabled) {
            accessibilityFocus = NONE;
            hovered = NONE;
        }
    }

    private static final class Record {
        final int[] v;
        final String[] s;
        Record(Object[] record) {
            v = (int[]) record[0];
            s = (String[]) record[1];
            if (v.length < HEADER || v[0] != 1 || v[26] < 0 || v[27] < 0
                    || v.length != HEADER + v[26] + v[27] || s.length != STRINGS + v[27]) {
                throw new IllegalStateException("Android accessibility record ABI mismatch");
            }
        }
        boolean flag(int bit) { return (v[10] & (1 << bit)) != 0; }
    }

    private Record read(int id) {
        Object[] record = MakepadNative.a11yNode(id);
        return record == null ? null : new Record(record);
    }

    @Override
    public AccessibilityNodeInfo createAccessibilityNodeInfo(int id) {
        Record r = read(id);
        if (id == HOST_VIEW_ID) {
            AccessibilityNodeInfo info = AccessibilityNodeInfo.obtain(host);
            host.onInitializeAccessibilityNodeInfo(info);
            // The semantic root is represented by the real SurfaceView, not an extra virtual node.
            if (r != null) for (int i = 0; i < r.v[26]; i++) info.addChild(host, r.v[HEADER + i]);
            return info;
        }
        if (r == null) return null;
        int[] v = r.v;
        AccessibilityNodeInfo info = AccessibilityNodeInfo.obtain();
        info.setSource(host, id);
        info.setPackageName(host.getContext().getPackageName());
        info.setClassName(r.s[0]);
        info.setParent(host, v[1]);
        info.setBoundsInParent(new Rect(v[2], v[3], v[4], v[5]));
        int[] location = new int[2];
        host.getLocationOnScreen(location);
        Rect screen = new Rect(v[6], v[7], v[8], v[9]);
        screen.offset(location[0], location[1]);
        info.setBoundsInScreen(screen);
        info.setText(r.s[1]);
        if (!r.s[2].isEmpty()) info.setContentDescription(r.s[2]);
        if (Build.VERSION.SDK_INT >= 26) {
            info.setHintText(r.s[3]);
            info.setShowingHintText(r.s[1].isEmpty() && !r.s[3].isEmpty());
        }
        if (Build.VERSION.SDK_INT >= 30) info.setStateDescription(r.s[4]);
        else if (!r.s[4].isEmpty()) info.setContentDescription(
            r.s[2].isEmpty() ? r.s[4] : r.s[2] + ", " + r.s[4]);
        if (!r.s[5].isEmpty()) info.setViewIdResourceName(host.getContext().getPackageName() + ":id/" + r.s[5]);
        info.setEnabled(r.flag(0));
        Rect visible = new Rect();
        info.setVisibleToUser(r.flag(1) && host.isShown() && host.getLocalVisibleRect(visible)
            && visible.intersect(v[6], v[7], v[8], v[9]));
        info.setFocusable(r.flag(2));
        info.setFocused(r.flag(3));
        info.setAccessibilityFocused(r.flag(4));
        info.setCheckable(r.flag(5));
        info.setChecked(r.flag(6));
        info.setSelected(r.flag(7));
        info.setPassword(r.flag(8));
        info.setEditable(r.flag(9));
        info.setScrollable(r.flag(10));
        info.setMultiLine(r.flag(11));
        if (Build.VERSION.SDK_INT >= 28) {
            info.setHeading(r.flag(12));
            info.setScreenReaderFocusable(r.flag(13));
        }
        if (v[11] >= 0 && v[12] >= 0) info.setTextSelection(v[11], v[12]);
        if (v[13] >= 0) info.setRangeInfo(AccessibilityNodeInfo.RangeInfo.obtain(
            v[13], Float.intBitsToFloat(v[14]), Float.intBitsToFloat(v[15]), Float.intBitsToFloat(v[16])));
        if (v[17] >= 0) info.setCollectionInfo(AccessibilityNodeInfo.CollectionInfo.obtain(v[17], v[18], v[19] != 0, v[20]));
        if (v[21] >= 0) info.setCollectionItemInfo(AccessibilityNodeInfo.CollectionItemInfo.obtain(
            v[21], v[22], v[23], v[24], r.flag(12), r.flag(7)));
        info.setLiveRegion(v[25]);
        for (int i = 0; i < v[26]; i++) info.addChild(host, v[HEADER + i]);
        for (int i = 0; i < v[27]; i++) {
            int action = v[HEADER + v[26] + i];
            info.addAction(new AccessibilityNodeInfo.AccessibilityAction(action, r.s[STRINGS + i].isEmpty() ? null : r.s[STRINGS + i]));
            if (action == AccessibilityNodeInfo.ACTION_CLICK) info.setClickable(true);
            if (action == AccessibilityNodeInfo.AccessibilityAction.ACTION_CONTEXT_CLICK.getId()) info.setContextClickable(true);
        }
        return info;
    }

    @Override
    public boolean performAction(int id, int action, Bundle args) {
        return performAction(id, action, args, false);
    }

    private boolean performAction(int id, int action, Bundle args, boolean wait) {
        if (id == HOST_VIEW_ID) return host.performAccessibilityAction(action, args);
        String text = null;
        int start = -1, end = -1;
        float value = Float.NaN;
        if (args != null) {
            CharSequence input = args.getCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE);
            if (input != null) text = input.toString();
            start = args.getInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, -1);
            end = args.getInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, -1);
            value = args.getFloat(AccessibilityNodeInfo.ACTION_ARGUMENT_PROGRESS_VALUE, Float.NaN);
        }
        if (action == AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS
                && (manager == null || !manager.isEnabled() || !manager.isTouchExplorationEnabled())) return false;
        if (action == AccessibilityNodeInfo.ACTION_SET_TEXT && text == null) return false;
        int oldFocus = accessibilityFocus;
        long request = MakepadNative.a11yAction(id, action, text, start, end, value, wait);
        if (request == 0) return false;
        lastRequest = request;
        if (action == AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS) {
            accessibilityFocus = id;
            if (oldFocus != NONE) sendEvent(oldFocus, AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED, 0, "");
            sendEvent(id, AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUSED, 0, "");
            host.invalidate();
        } else if (action == AccessibilityNodeInfo.ACTION_CLEAR_ACCESSIBILITY_FOCUS) {
            accessibilityFocus = NONE;
            sendEvent(id, AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED, 0, "");
            host.invalidate();
        }
        return true;
    }

    @Override
    public AccessibilityNodeInfo findFocus(int focus) {
        int id = MakepadNative.a11yFindFocus(focus);
        return id == NONE ? null : createAccessibilityNodeInfo(id);
    }

    @Override
    public List<AccessibilityNodeInfo> findAccessibilityNodeInfosByText(String text, int root) {
        List<AccessibilityNodeInfo> found = new ArrayList<>();
        if (text == null) return found;
        String query = text.toLowerCase(Locale.ROOT);
        ArrayDeque<Integer> pending = new ArrayDeque<>();
        HashSet<Integer> seen = new HashSet<>();
        pending.add(root);
        while (!pending.isEmpty()) {
            int id = pending.removeFirst();
            if (!seen.add(id)) continue;
            Record r = read(id);
            if (r == null) continue;
            if (id != HOST_VIEW_ID && (r.s[1].toLowerCase(Locale.ROOT).contains(query)
                    || r.s[2].toLowerCase(Locale.ROOT).contains(query))) {
                AccessibilityNodeInfo info = createAccessibilityNodeInfo(id);
                if (info != null) found.add(info);
            }
            for (int i = 0; i < r.v[26]; i++) pending.add(r.v[HEADER + i]);
        }
        return found;
    }

    boolean dispatchHoverEvent(MotionEvent event) {
        if (manager == null || !manager.isEnabled() || !manager.isTouchExplorationEnabled()) return false;
        int action = event.getActionMasked();
        if (action != MotionEvent.ACTION_HOVER_ENTER && action != MotionEvent.ACTION_HOVER_MOVE
                && action != MotionEvent.ACTION_HOVER_EXIT) return false;
        int id = action == MotionEvent.ACTION_HOVER_EXIT ? NONE : MakepadNative.a11yHitTest(event.getX(), event.getY());
        if (id == HOST_VIEW_ID) id = NONE;
        int old = hovered;
        if (old != id) {
            hovered = id;
            if (id != NONE) sendEvent(id, AccessibilityEvent.TYPE_VIEW_HOVER_ENTER, 0, "");
            if (old != NONE) sendEvent(old, AccessibilityEvent.TYPE_VIEW_HOVER_EXIT, 0, "");
        }
        return id != NONE || old != NONE;
    }

    void postEvent(int id, int kind, int change, String text) {
        // Native updates arrive on the notification worker, never Android's main looper.
        main.post(() -> {
            if (!attached) return;
            if (accessibilityFocus != NONE && MakepadNative.a11yFindFocus(AccessibilityNodeInfo.FOCUS_ACCESSIBILITY) != accessibilityFocus) {
                int old = accessibilityFocus;
                accessibilityFocus = NONE;
                sendEvent(old, AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED, 0, "");
            }
            sendEvent(id, kind, change, text);
        });
    }

    private void sendEvent(int id, int kind, int change, String announcement) {
        if (manager == null || !manager.isEnabled() || host.getParent() == null) return;
        AccessibilityEvent event = AccessibilityEvent.obtain(kind);
        event.setPackageName(host.getContext().getPackageName());
        event.setSource(host, id);
        Record r = read(id);
        if (r != null) {
            event.setClassName(r.s[0]);
            event.setEnabled(r.flag(0));
            event.setChecked(r.flag(6));
            event.setPassword(r.flag(8));
            if (!r.s[1].isEmpty()) event.getText().add(r.s[1]);
            if (!r.s[2].isEmpty()) event.setContentDescription(r.s[2]);
            if (kind == AccessibilityEvent.TYPE_VIEW_TEXT_SELECTION_CHANGED) {
                event.setFromIndex(r.v[11]);
                event.setToIndex(r.v[12]);
                event.setItemCount(r.s[1].length());
            }
        }
        if (kind == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED) {
            event.setContentChangeTypes(Build.VERSION.SDK_INT >= 30 || change != 64 ? change : AccessibilityEvent.CONTENT_CHANGE_TYPE_UNDEFINED);
        }
        if (announcement != null && !announcement.isEmpty()) {
            event.getText().clear();
            event.getText().add(announcement);
        }
        ViewParent parent = host.getParent();
        parent.requestSendAccessibilityEvent(host, event);
    }

    // Called only on the main looper by the remote OS check. It exercises the public provider
    // entry point, then the Rust check waits for the shared ActionDone completion off the UI thread.
    long selfCheckAction(int id, int action) {
        lastRequest = 0;
        return performAction(id, action, null, true) ? lastRequest : 0;
    }

    String selfCheck() {
        int count = 0;
        ArrayDeque<Integer> pending = new ArrayDeque<>();
        HashSet<Integer> seen = new HashSet<>();
        pending.add(HOST_VIEW_ID);
        while (!pending.isEmpty()) {
            int id = pending.removeFirst();
            if (!seen.add(id)) return "error: Android provider cycle";
            Record r = read(id);
            AccessibilityNodeInfo info = createAccessibilityNodeInfo(id);
            if (r == null || info == null) return "error: Android provider missing id=" + id;
            String error = null;
            if (info.getChildCount() != r.v[26]) error = "children";
            if (id != HOST_VIEW_ID) {
                Rect bounds = new Rect();
                info.getBoundsInParent(bounds);
                if (!bounds.equals(new Rect(r.v[2], r.v[3], r.v[4], r.v[5]))) error = "parent bounds";
                int[] location = new int[2];
                host.getLocationOnScreen(location);
                info.getBoundsInScreen(bounds);
                if (!bounds.equals(new Rect(r.v[6] + location[0], r.v[7] + location[1], r.v[8] + location[0], r.v[9] + location[1]))) error = "screen bounds";
                if (!r.s[0].contentEquals(info.getClassName()) || !r.s[1].contentEquals(info.getText())) error = "class/text";
                if (info.isEnabled() != r.flag(0) || info.isCheckable() != r.flag(5)
                        || info.isChecked() != r.flag(6) || info.isFocused() != r.flag(3)
                        || info.isAccessibilityFocused() != r.flag(4) || info.isEditable() != r.flag(9)) error = "states";
                if (info.getTextSelectionStart() != r.v[11] || info.getTextSelectionEnd() != r.v[12]) error = "selection";
                if (info.getActionList().size() != r.v[27]) error = "actions";
                if (r.v[13] >= 0 && (info.getRangeInfo() == null
                        || info.getRangeInfo().getCurrent() != Float.intBitsToFloat(r.v[16]))) error = "range";
            }
            info.recycle();
            if (error != null) return "error: Android provider " + error + " id=" + id;
            for (int i = 0; i < r.v[26]; i++) pending.add(r.v[HEADER + i]);
            count++;
        }
        return "ok: Java AccessibilityNodeProvider nodes=" + count;
    }
}
