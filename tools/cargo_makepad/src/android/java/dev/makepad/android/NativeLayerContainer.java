package dev.makepad.android;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Outline;
import android.graphics.Rect;
import android.os.Build;
import android.view.MotionEvent;
import android.view.SurfaceControl;
import android.view.Surface;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewOutlineProvider;
import android.view.inputmethod.InputMethodManager;
import android.webkit.WebChromeClient;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;

/** Geometry and hit regions are replaced together when a rendered frame is committed. */
final class NativeLayerContainer extends FrameLayout {
    // Enable before launch: adb shell setprop log.tag.MakepadLayers VERBOSE.
    private static final boolean TRACE = android.util.Log.isLoggable("MakepadLayers", android.util.Log.VERBOSE);
    private long drawSerial;
    private final MakepadSurface surface;
    private final HashMap<String, Page> pages = new HashMap<>();
    private final ArrayList<Page> order = new ArrayList<>();
    private final SurfaceControl[] segments = new SurfaceControl[5];
    private final Surface[] bufferSurfaces = new Surface[5];
    private SurfaceView upperParent;
    private View gesture;
    private boolean closed;
    private long lastSerial;
    private boolean loggedFrame;
    private SurfaceControl.Transaction pendingDrawTransaction;
    private boolean drawRetryPosted;

    private static String key(int session, long id) { return session + ":" + id; }

    private final class Page {
        final int session;
        final long id;
        final WebView web;
        float[] geometry;
        float radius;
        Page(int session, long id) {
            this.session = session;
            this.id = id;
            web = new WebView(getContext());
            web.setVisibility(INVISIBLE);
            web.setAccessibilityTraversalAfter(surface.getId());
            web.getSettings().setJavaScriptEnabled(true);
            web.getSettings().setDomStorageEnabled(true);
            web.setWebViewClient(new WebViewClient() {
                @Override public void onPageStarted(WebView view, String url, android.graphics.Bitmap icon) {
                    emit(3, 0, 0, url); emit(4, 1, 0, ""); navigation();
                }
                @Override public void onPageFinished(WebView view, String url) {
                    emit(3, 0, 0, url); emit(4, 0, 0, ""); navigation();
                }
                @Override public void doUpdateVisitedHistory(WebView view, String url, boolean reload) {
                    emit(3, 0, 0, url); navigation();
                }
                @Override public boolean onRenderProcessGone(WebView view, android.webkit.RenderProcessGoneDetail detail) {
                    emit(9, 0, 0, detail.didCrash() ? "Web renderer crashed" : "Web renderer stopped");
                    removeView(web); web.destroy(); pages.remove(key(session, id)); order.remove(Page.this);
                    return true;
                }
            });
            web.setWebChromeClient(new WebChromeClient() {
                @Override public void onReceivedTitle(WebView view, String title) { emit(2, 0, 0, title); }
                @Override public void onProgressChanged(WebView view, int progress) { emit(4, progress < 100 ? 1 : 0, 0, ""); }
            });
            web.setOnFocusChangeListener((view, focused) -> emit(focused ? 6 : 7, 0, 0, ""));
            web.setOutlineProvider(new ViewOutlineProvider() {
                @Override public void getOutline(View view, Outline outline) {
                    outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), radius);
                }
            });
            web.setClipToOutline(true);
            addView(web, new FrameLayout.LayoutParams(1, 1));
        }
        void emit(int what, long a, long b, String text) {
            MakepadNative.onNativeLayerState(session, id, what, a, b, text == null ? "" : text);
        }
        void navigation() { emit(5, web.canGoBack() ? 1 : 0, web.canGoForward() ? 1 : 0, ""); }
        boolean hit(float x, float y) {
            if (geometry == null || web.getVisibility() != VISIBLE || (((int)geometry[13]) & 2) != 0) return false;
            if (!inside(x, y, geometry, 0) || !inside(x, y, geometry, 4)) return false;
            for (int i = 15; i + 3 < geometry.length; i += 4) if (inside(x, y, geometry, i)) return false;
            // The outline clips the corners too; do not send taps in those transparent pixels.
            float r = Math.min(radius, Math.min(geometry[2], geometry[3]) * .5f);
            float dx = Math.max(0, Math.max(geometry[0] + r - x, x - (geometry[0] + geometry[2] - r)));
            float dy = Math.max(0, Math.max(geometry[1] + r - y, y - (geometry[1] + geometry[3] - r)));
            return dx * dx + dy * dy <= r * r;
        }
    }

    NativeLayerContainer(Context context, MakepadSurface surface) {
        super(context);
        this.surface = surface;
        setClipChildren(false);
        setClipToPadding(false);
        setLayoutParams(new FrameLayout.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        if (TRACE) getViewTreeObserver().addOnPreDrawListener(() -> {
            long draw = ++drawSerial, serial = lastSerial;
            android.util.Log.v("MakepadLayers", "draw=" + draw + " serial=" + serial);
            if (Build.VERSION.SDK_INT >= 29 && isHardwareAccelerated()) getViewTreeObserver().registerFrameCommitCallback(() ->
                android.util.Log.v("MakepadLayers", "hwui-commit draw=" + draw + " serial=" + serial));
            return true;
        });
    }

    private static boolean inside(float x, float y, float[] r, int i) {
        return x >= r[i] && y >= r[i+1] && x < r[i] + r[i+2] && y < r[i+1] + r[i+3];
    }

    boolean ownsFocus() {
        for (Page page : pages.values()) if (page.web.hasFocus()) return true;
        return false;
    }

    void create(int session, long id, byte[] url) {
        if (closed) return;
        destroy(session, id);
        Page page = new Page(session, id);
        pages.put(key(session, id), page);
        page.emit(1, 1, 0, "");
        page.web.loadUrl(new String(url, StandardCharsets.UTF_8));
    }

    void operation(int session, long id, int op, long arg, byte[] data) {
        Page page = pages.get(key(session, id));
        if (page == null) return;
        WebView web = page.web;
        switch (op) {
            case 1:
                String url = new String(data, StandardCharsets.UTF_8);
                if (arg != 0) web.evaluateJavascript("location.replace(" + org.json.JSONObject.quote(url) + ")", null);
                else web.loadUrl(url);
                break;
            case 2: web.goBackOrForward((int)arg); break;
            case 3: web.reload(); break;
            case 4: web.stopLoading(); page.emit(4, 0, 0, ""); break;
            case 5:
                if (data.length == 16) {
                    ByteBuffer b = ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN);
                    float r = b.getFloat(), g = b.getFloat(), blue = b.getFloat(), a = b.getFloat();
                    web.setBackgroundColor(Color.argb(channel(a), channel(r), channel(g), channel(blue)));
                }
                if (Build.VERSION.SDK_INT >= 29) web.getSettings().setForceDark((arg & 1) != 0 ? WebSettings.FORCE_DARK_ON : WebSettings.FORCE_DARK_OFF);
                if (Build.VERSION.SDK_INT >= 33) web.getSettings().setAlgorithmicDarkeningAllowed((arg & 1) != 0);
                break;
            case 6: web.requestFocus(); break;
            case 7:
                if (web.hasFocus()) {
                    InputMethodManager imm = (InputMethodManager)getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
                    if (imm != null) imm.hideSoftInputFromWindow(web.getWindowToken(), 0);
                    web.clearFocus(); surface.requestFocus();
                }
                break;
        }
    }

    private static int channel(float value) { return Math.round(Math.max(0, Math.min(1, value)) * 255); }

    void destroy(int session, long id) {
        Page page = pages.remove(key(session, id));
        if (page == null) return;
        if (gesture == page.web) cancelGesture();
        if (page.web.hasFocus()) { page.web.clearFocus(); surface.requestFocus(); }
        order.remove(page); removeView(page.web); page.web.destroy(); page.emit(10, 0, 0, "");
    }

    private void cancelGesture() {
        if (gesture == null) return;
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0, 0, 0);
        gesture.dispatchTouchEvent(cancel); cancel.recycle(); gesture = null;
    }

    // Capture is selected once on DOWN. Crossing a popup or a moving layer never retargets it.
    @Override public boolean onInterceptTouchEvent(MotionEvent event) { return true; }
    @Override public boolean dispatchTouchEvent(MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
            gesture = surface;
            int[] here = new int[2], base = new int[2];
            getLocationInWindow(here); surface.getLocationInWindow(base);
            float x = event.getX() + here[0] - base[0], y = event.getY() + here[1] - base[1];
            for (int i = order.size() - 1; i >= 0; --i) {
                if (order.get(i).hit(x, y)) { gesture = order.get(i).web; break; }
            }
        }
        if (gesture == null) return false;
        int[] here = new int[2], there = new int[2];
        getLocationInWindow(here); gesture.getLocationInWindow(there);
        MotionEvent copy = MotionEvent.obtain(event);
        copy.offsetLocation(here[0] - there[0], here[1] - there[1]);
        gesture.dispatchTouchEvent(copy); copy.recycle();
        if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL) gesture = null;
        return true;
    }

    /** The NDK fills the Java transaction before geometry changes, then HWUI owns its apply. */
    void frame(long ticket, long serial, int session, long[] ids, float[] geometry, int segmentCount) {
        if (TRACE) android.util.Log.v("MakepadLayers", "frame serial=" + serial + " ticket=" + ticket
            + " api=" + Build.VERSION.SDK_INT + " segments=" + segmentCount + " hw=" + isHardwareAccelerated()
            + " y=" + (geometry.length > 1 ? geometry[1] : 0));
        if (!loggedFrame) { loggedFrame = true; android.util.Log.i("MakepadLayers", "received frame: " + ids.length + " web views, " + segmentCount + " segments"); }
        if (closed || serial < lastSerial) { MakepadNative.discardNativeLayerFrame(ticket); return; }
        if (!isAttachedToWindow() || !surface.getHolder().getSurface().isValid()) {
            postOnAnimation(() -> frame(ticket, serial, session, ids, geometry, segmentCount)); return;
        }
        SurfaceControl.Transaction tx = null;
        if (Build.VERSION.SDK_INT >= 29 && segmentCount > 0) {
            if ((Build.VERSION.SDK_INT >= 31 && getRootSurfaceControl() == null) || surface.getSurfaceControl() == null || !surface.getSurfaceControl().isValid()) {
                postOnAnimation(() -> frame(ticket, serial, session, ids, geometry, segmentCount)); return;
            }
            if (Build.VERSION.SDK_INT < 31) {
                // AttachedSurfaceControl is public from 31. Earlier releases need a transparent
                // SurfaceView to supply the above-window parent; all five buffers still use NDK transactions.
                if (upperParent == null) {
                    upperParent = new SurfaceView(getContext()); upperParent.setZOrderOnTop(true);
                    upperParent.getHolder().setFormat(android.graphics.PixelFormat.TRANSLUCENT);
                    upperParent.setClickable(false); upperParent.setFocusable(false);
                    addView(upperParent, new FrameLayout.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
                }
                if (upperParent.getSurfaceControl() == null || !upperParent.getSurfaceControl().isValid()) {
                    postOnAnimation(() -> frame(ticket, serial, session, ids, geometry, segmentCount)); return;
                }
            }
            tx = new SurfaceControl.Transaction();
            for (int i = 0; i < segments.length; i++) {
                if (segments[i] == null) {
                    SurfaceControl.Builder builder = new SurfaceControl.Builder().setName("Makepad segment " + i).setBufferSize(1, 1).setOpaque(i == 0);
                    if (i == 0) builder.setParent(surface.getSurfaceControl());
                    else if (Build.VERSION.SDK_INT < 31) builder.setParent(upperParent.getSurfaceControl());
                    segments[i] = builder.build();
                    if (i != 0 && Build.VERSION.SDK_INT >= 31) {
                        SurfaceControl.Transaction reparent = getRootSurfaceControl().buildReparentTransaction(segments[i]);
                        tx.merge(reparent); reparent.close();
                    }
                    if (Build.VERSION.SDK_INT < 34) bufferSurfaces[i] = new Surface(segments[i]);
                }
                if (Build.VERSION.SDK_INT < 34) tx.setLayer(segments[i], i == 0 ? 1 : i).setVisibility(segments[i], true);
            }
        } else if (Build.VERSION.SDK_INT >= 34 && segments[0] != null) {
            tx = new SurfaceControl.Transaction();
        }
        if (tx != null && Build.VERSION.SDK_INT < 34) { tx.apply(); tx.close(); tx = null; }
        Object[] handles = Build.VERSION.SDK_INT >= 34 ? segments : Build.VERSION.SDK_INT >= 29 ? bufferSurfaces : null;
        int[] surfaceOrigin = new int[2];
        if (Build.VERSION.SDK_INT >= 29) surface.getLocationInSurface(surfaceOrigin);
        if (!MakepadNative.commitNativeLayerFrame(ticket, tx, handles, surfaceOrigin[0], surfaceOrigin[1])) {
            if (tx != null) tx.close();
            postOnAnimation(() -> frame(ticket, serial, session, ids, geometry, segmentCount)); return;
        }
        lastSerial = serial;
        if (TRACE) android.util.Log.v("MakepadLayers", "transaction serial=" + serial + " ticket=" + ticket
            + " tx=" + (tx == null ? "direct" : Integer.toHexString(System.identityHashCode(tx))));
        ArrayList<Page> previous = new ArrayList<>(order);
        order.clear();
        for (Page page : previous) page.geometry = null;
        int at = 0;
        int[] base = new int[2], here = new int[2];
        surface.getLocationInWindow(base); getLocationInWindow(here);
        for (long id : ids) {
            int length = 15 + (int)geometry[at + 14] * 4;
            Page page = pages.get(key(session, id));
            if (page != null) {
                float[] g = java.util.Arrays.copyOfRange(geometry, at, at + length);
                page.geometry = g;
                int width = Math.max(1, (int)Math.ceil(g[2])), height = Math.max(1, (int)Math.ceil(g[3]));
                FrameLayout.LayoutParams params = (FrameLayout.LayoutParams)page.web.getLayoutParams();
                if (params.width != width || params.height != height) { params.width = width; params.height = height; page.web.setLayoutParams(params); }
                page.web.setTranslationX(base[0] - here[0] + g[0]); page.web.setTranslationY(base[1] - here[1] + g[1]);
                page.web.setClipBounds(new Rect((int)Math.ceil(g[4]-g[0]), (int)Math.ceil(g[5]-g[1]), (int)Math.floor(g[4]+g[6]-g[0]), (int)Math.floor(g[5]+g[7]-g[1])));
                float radius = Math.max(Math.max(g[8], g[9]), Math.max(g[10], g[11]));
                if (TRACE) android.util.Log.v("MakepadLayers", "geometry serial=" + serial + " y=" + g[1]
                    + " height=" + height + " radius=" + radius + " scrollY=" + page.web.getScrollY());
                if (page.radius != radius) { page.radius = radius; page.web.invalidateOutline(); }
                page.web.setAlpha(g[12]);
                page.web.setVisibility((((int)g[13]) & 1) != 0 && g[6] > 0 && g[7] > 0 ? VISIBLE : INVISIBLE);
                order.add(page);
            }
            at += length;
        }
        for (Page page : previous) if (page.geometry == null) page.web.setVisibility(INVISIBLE);
        if (!previous.equals(order)) for (Page page : order) page.web.bringToFront();
        if (tx != null) {
            if (pendingDrawTransaction == null) pendingDrawTransaction = tx;
            else { pendingDrawTransaction.merge(tx); tx.close(); }
            applyOnDraw();
        }
        invalidate();
    }

    private void applyOnDraw() {
        if (Build.VERSION.SDK_INT < 34 || pendingDrawTransaction == null) return;
        if (isAttachedToWindow() && getRootSurfaceControl() != null
                && getRootSurfaceControl().applyTransactionOnDraw(pendingDrawTransaction)) {
            pendingDrawTransaction.close(); pendingDrawTransaction = null;
            if (TRACE) android.util.Log.v("MakepadLayers", "apply-on-draw serial=" + lastSerial
                + " nextDraw=" + (drawSerial + 1) + " hw=" + isHardwareAccelerated());
        } else if (!drawRetryPosted && !closed) {
            drawRetryPosted = true;
            postOnAnimation(() -> { drawRetryPosted = false; applyOnDraw(); });
        }
        invalidate();
    }

    void close() {
        closed = true; cancelGesture();
        for (Page page : new ArrayList<>(pages.values())) destroy(page.session, page.id);
        surfaceLost();
    }

    void surfaceLost() {
        if (Build.VERSION.SDK_INT >= 29) {
            // Finish a transaction queued during detachment before clearing its buffers. This
            // runs only in teardown; visible frames on 34+ always go through applyOnDraw.
            if (pendingDrawTransaction != null) {
                pendingDrawTransaction.apply(); pendingDrawTransaction.close(); pendingDrawTransaction = null;
            }
            MakepadNative.releaseNativeLayerSurfaces(Build.VERSION.SDK_INT >= 34 ? segments : bufferSurfaces);
            try (SurfaceControl.Transaction tx = new SurfaceControl.Transaction()) {
                for (int i = 0; i < segments.length; i++) if (segments[i] != null) { tx.reparent(segments[i], null); segments[i].release(); segments[i] = null; }
                tx.apply();
            }
            for (int i = 0; i < bufferSurfaces.length; i++) if (bufferSurfaces[i] != null) { bufferSurfaces[i].release(); bufferSurfaces[i] = null; }
        }
    }
}
