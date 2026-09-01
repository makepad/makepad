package dev.makepad.android;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.speech.RecognitionListener;
import android.speech.RecognizerIntent;
import android.speech.SpeechRecognizer;
import android.speech.tts.TextToSpeech;
import android.speech.tts.UtteranceProgressListener;
import android.speech.tts.Voice;
import android.util.Log;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Locale;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

// The OS speech engines for makepad-system-speech. Both of them insist on the
// main looper (TextToSpeech's service connection and every SpeechRecognizer
// method), while the Rust side calls from worker threads and blocks; so the
// work is posted to the main looper and the caller parks on a CountDownLatch.
//
// Minimum API level is 26: SpeechRecognizer.createSpeechRecognizer +
// EXTRA_PREFER_OFFLINE (API 23), not createOnDeviceSpeechRecognizer (API 31);
// synthesizeToFile(CharSequence, Bundle, File, String) (API 21), not the
// ParcelFileDescriptor overload (API 30).
public class MakepadSpeech {
    private static final String TAG = "Makepad";
    private static final long TTS_INIT_TIMEOUT_S = 10;
    private static final long TTS_RENDER_TIMEOUT_S = 60;

    private final Activity mActivity;
    private final Handler mMainHandler;

    // One engine, so one utterance at a time.
    private final Object mTtsLock = new Object();
    private volatile TextToSpeech mTts;
    private volatile String mTtsLastError = "";
    private final AtomicLong mUtteranceCounter = new AtomicLong(1);
    private volatile CountDownLatch mUtteranceLatch;
    private volatile String mUtteranceId;
    private volatile boolean mUtteranceOk;

    // Touched only on the main looper, so it needs no lock.
    private final HashMap<Long, SpeechRecognizer> mRecognizers = new HashMap<>();

    public MakepadSpeech(Activity activity) {
        mActivity = activity;
        mMainHandler = new Handler(Looper.getMainLooper());
    }

    public static native void onSttEvent(long session, int kind, String text, float level);

    // ------------------------------------------------------------------- TTS

    public String ttsLastError() {
        return mTtsLastError;
    }

    public boolean ttsAvailable() {
        return ensureTts() != null;
    }

    public String[] ttsVoices() {
        TextToSpeech tts = ensureTts();
        if (tts == null) {
            return new String[0];
        }
        ArrayList<String> out = new ArrayList<>();
        try {
            Set<Voice> voices = tts.getVoices();
            if (voices != null) {
                for (Voice voice : voices) {
                    if (voice == null || voice.getName() == null) {
                        continue;
                    }
                    Locale locale = voice.getLocale();
                    String tag = locale == null ? "" : locale.toLanguageTag();
                    out.add(voice.getName() + "\t" + tag + "\t" + voice.getQuality()
                        + "\t" + voice.isNetworkConnectionRequired());
                }
            }
        }
        catch (Exception e) {
            mTtsLastError = "ttsVoices: " + e.toString();
            Log.e(TAG, "ttsVoices: " + e.toString());
        }
        return out.toArray(new String[0]);
    }

    public byte[] ttsSynthesize(String text, String voiceName, String languageTag, float rate, float pitch) {
        TextToSpeech tts = ensureTts();
        if (tts == null) {
            return null;
        }
        synchronized (mTtsLock) {
            File file = null;
            try {
                if (languageTag != null && !languageTag.isEmpty()) {
                    int result = tts.setLanguage(Locale.forLanguageTag(languageTag));
                    if (result == TextToSpeech.LANG_MISSING_DATA || result == TextToSpeech.LANG_NOT_SUPPORTED) {
                        // Not fatal: the engine keeps its previous language and
                        // still renders, so report it only if the render fails.
                        mTtsLastError = "language " + languageTag + " unavailable";
                    }
                }
                if (voiceName != null && !voiceName.isEmpty()) {
                    Set<Voice> voices = tts.getVoices();
                    if (voices != null) {
                        for (Voice voice : voices) {
                            if (voice != null && voiceName.equals(voice.getName())) {
                                tts.setVoice(voice);
                                break;
                            }
                        }
                    }
                }
                tts.setSpeechRate(rate);
                tts.setPitch(pitch);

                String utteranceId = "makepad-" + mUtteranceCounter.getAndIncrement();
                CountDownLatch done = new CountDownLatch(1);
                mUtteranceId = utteranceId;
                mUtteranceOk = false;
                mUtteranceLatch = done;

                file = File.createTempFile("makepad-tts", ".wav", mActivity.getCacheDir());
                int queued = tts.synthesizeToFile(text, new Bundle(), file, utteranceId);
                if (queued != TextToSpeech.SUCCESS) {
                    mTtsLastError = "synthesizeToFile refused the utterance";
                    return null;
                }
                if (!done.await(TTS_RENDER_TIMEOUT_S, TimeUnit.SECONDS)) {
                    mTtsLastError = "tts render timed out after " + TTS_RENDER_TIMEOUT_S + "s";
                    return null;
                }
                if (!mUtteranceOk) {
                    return null;
                }
                byte[] bytes = readAll(file);
                if (bytes == null || bytes.length == 0) {
                    mTtsLastError = "tts wrote no audio";
                    return null;
                }
                return bytes;
            }
            catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                mTtsLastError = "interrupted while rendering";
                return null;
            }
            catch (Exception e) {
                mTtsLastError = "ttsSynthesize: " + e.toString();
                Log.e(TAG, "ttsSynthesize: " + e.toString());
                return null;
            }
            finally {
                mUtteranceLatch = null;
                mUtteranceId = null;
                if (file != null) {
                    file.delete();
                }
            }
        }
    }

    // Called from onDestroy on the main thread, so it must not take mTtsLock: a
    // render in flight can hold that for a minute, and blocking the UI thread
    // that long is an ANR.
    public void shutdown() {
        TextToSpeech tts = mTts;
        mTts = null;
        if (tts != null) {
            try {
                tts.stop();
                tts.shutdown();
            }
            catch (Exception e) {
                Log.e(TAG, "tts shutdown: " + e.toString());
            }
        }
        mTtsLastError = "the speech engine was shut down";
        CountDownLatch pending = mUtteranceLatch;
        if (pending != null) {
            mUtteranceOk = false;
            pending.countDown();
        }
        mMainHandler.post(new Runnable() {
            @Override public void run() {
                for (Long session : new ArrayList<>(mRecognizers.keySet())) {
                    finishSession(session);
                }
            }
        });
    }

    // TextToSpeech delivers onInit on the main looper, so the engine is built
    // there and the calling worker parks on a latch. Calling this from the main
    // thread would wait for a callback that can only run once we return.
    private TextToSpeech ensureTts() {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            mTtsLastError = "system speech must be used from a worker thread";
            return null;
        }
        synchronized (mTtsLock) {
            if (mTts != null) {
                return mTts;
            }
            final CountDownLatch ready = new CountDownLatch(1);
            final int[] status = new int[]{ TextToSpeech.ERROR };
            final TextToSpeech[] engine = new TextToSpeech[1];
            mMainHandler.post(new Runnable() {
                @Override public void run() {
                    try {
                        engine[0] = new TextToSpeech(mActivity, new TextToSpeech.OnInitListener() {
                            @Override public void onInit(int code) {
                                status[0] = code;
                                ready.countDown();
                            }
                        });
                    }
                    catch (Exception e) {
                        Log.e(TAG, "tts create: " + e.toString());
                        ready.countDown();
                    }
                }
            });
            try {
                if (!ready.await(TTS_INIT_TIMEOUT_S, TimeUnit.SECONDS)) {
                    mTtsLastError = "no text-to-speech engine answered within " + TTS_INIT_TIMEOUT_S + "s";
                    return null;
                }
            }
            catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                mTtsLastError = "interrupted waiting for the tts engine";
                return null;
            }
            if (status[0] != TextToSpeech.SUCCESS || engine[0] == null) {
                mTtsLastError = "no text-to-speech engine installed (init status " + status[0] + ")";
                final TextToSpeech dead = engine[0];
                if (dead != null) {
                    mMainHandler.post(new Runnable() {
                        @Override public void run() { dead.shutdown(); }
                    });
                }
                return null;
            }
            engine[0].setOnUtteranceProgressListener(new UtteranceProgressListener() {
                @Override public void onStart(String utteranceId) {}
                @Override public void onDone(String utteranceId) {
                    finishUtterance(utteranceId, true, null);
                }
                // Abstract in the base class even though it is deprecated.
                @Override public void onError(String utteranceId) {
                    finishUtterance(utteranceId, false, "tts engine error");
                }
                // API 21; the base implementation forwards to onError(String),
                // which this overrides away so the utterance finishes once.
                @Override public void onError(String utteranceId, int errorCode) {
                    finishUtterance(utteranceId, false, "tts engine error " + errorCode);
                }
            });
            mTts = engine[0];
            return mTts;
        }
    }

    private void finishUtterance(String utteranceId, boolean ok, String error) {
        CountDownLatch latch = mUtteranceLatch;
        if (latch == null || utteranceId == null || !utteranceId.equals(mUtteranceId)) {
            return;
        }
        if (!ok && error != null) {
            mTtsLastError = error;
        }
        mUtteranceOk = ok;
        latch.countDown();
    }

    private static byte[] readAll(File file) {
        FileInputStream in = null;
        try {
            in = new FileInputStream(file);
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            byte[] chunk = new byte[64 * 1024];
            int read;
            while ((read = in.read(chunk)) > 0) {
                out.write(chunk, 0, read);
            }
            return out.toByteArray();
        }
        catch (Exception e) {
            Log.e(TAG, "reading rendered speech: " + e.toString());
            return null;
        }
        finally {
            if (in != null) {
                try { in.close(); } catch (Exception ignored) {}
            }
        }
    }

    // ------------------------------------------------------------------- STT

    public boolean sttAvailable() {
        try {
            return SpeechRecognizer.isRecognitionAvailable(mActivity);
        }
        catch (Exception e) {
            return false;
        }
    }

    public void sttStart(final long session, final String languageTag, final boolean partial, final boolean preferOffline) {
        mMainHandler.post(new Runnable() {
            @Override public void run() {
                if (mRecognizers.containsKey(session)) {
                    return;
                }
                try {
                    if (!SpeechRecognizer.isRecognitionAvailable(mActivity)) {
                        onSttEvent(session, 3, "client", 0.0f);
                        onSttEvent(session, 4, null, 0.0f);
                        return;
                    }
                    SpeechRecognizer recognizer = SpeechRecognizer.createSpeechRecognizer(mActivity);
                    mRecognizers.put(session, recognizer);
                    recognizer.setRecognitionListener(new SessionListener(session));

                    Intent intent = new Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH);
                    intent.putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM);
                    intent.putExtra(RecognizerIntent.EXTRA_LANGUAGE, languageTag);
                    intent.putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, partial);
                    // API 23. A request, not a promise: an engine with no
                    // on-device model still recognizes over the network.
                    intent.putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, preferOffline);
                    // Some engines reject a session without a calling package.
                    intent.putExtra(RecognizerIntent.EXTRA_CALLING_PACKAGE, mActivity.getPackageName());
                    recognizer.startListening(intent);
                }
                catch (Exception e) {
                    Log.e(TAG, "sttStart: " + e.toString());
                    onSttEvent(session, 3, "client", 0.0f);
                    finishSession(session);
                }
            }
        });
    }

    public void sttStop(final long session) {
        mMainHandler.post(new Runnable() {
            @Override public void run() {
                SpeechRecognizer recognizer = mRecognizers.get(session);
                if (recognizer == null) {
                    return;
                }
                try {
                    // The final result still arrives, through onResults.
                    recognizer.stopListening();
                }
                catch (Exception e) {
                    Log.e(TAG, "sttStop: " + e.toString());
                    finishSession(session);
                }
            }
        });
    }

    // Exactly one Ended per session: the map entry is the token, and only the
    // call that removes it reports the end.
    private void finishSession(long session) {
        SpeechRecognizer recognizer = mRecognizers.remove(session);
        if (recognizer == null) {
            return;
        }
        try {
            recognizer.destroy();
        }
        catch (Exception e) {
            Log.e(TAG, "recognizer destroy: " + e.toString());
        }
        onSttEvent(session, 4, null, 0.0f);
    }

    private static String firstResult(Bundle results) {
        if (results == null) {
            return "";
        }
        ArrayList<String> texts = results.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION);
        if (texts == null || texts.isEmpty() || texts.get(0) == null) {
            return "";
        }
        return texts.get(0);
    }

    private class SessionListener implements RecognitionListener {
        private final long mSession;

        SessionListener(long session) {
            mSession = session;
        }

        @Override public void onReadyForSpeech(Bundle params) {}
        @Override public void onBeginningOfSpeech() {}
        @Override public void onBufferReceived(byte[] buffer) {}
        @Override public void onEndOfSpeech() {}
        @Override public void onEvent(int eventType, Bundle params) {}

        @Override public void onRmsChanged(float rmsdB) {
            // The framework documents no range; in practice it spans roughly
            // -2 dB (silence) to 10 dB (loud), so normalize across that.
            float level = (rmsdB + 2.0f) / 12.0f;
            if (level < 0.0f) level = 0.0f;
            if (level > 1.0f) level = 1.0f;
            onSttEvent(mSession, 0, null, level);
        }

        @Override public void onPartialResults(Bundle partialResults) {
            String text = firstResult(partialResults);
            if (!text.isEmpty()) {
                onSttEvent(mSession, 1, text, 0.0f);
            }
        }

        @Override public void onResults(Bundle results) {
            onSttEvent(mSession, 2, firstResult(results), 0.0f);
            finishSession(mSession);
        }

        @Override public void onError(int code) {
            String word;
            switch (code) {
                case SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS: word = "permission"; break;
                case SpeechRecognizer.ERROR_NETWORK: word = "network"; break;
                case SpeechRecognizer.ERROR_NETWORK_TIMEOUT: word = "network"; break;
                case SpeechRecognizer.ERROR_SERVER: word = "server"; break;
                case SpeechRecognizer.ERROR_AUDIO: word = "audio"; break;
                case SpeechRecognizer.ERROR_RECOGNIZER_BUSY: word = "busy"; break;
                case SpeechRecognizer.ERROR_NO_MATCH: word = "nomatch"; break;
                case SpeechRecognizer.ERROR_SPEECH_TIMEOUT: word = "timeout"; break;
                default: word = "client"; break;
            }
            if (word.equals("nomatch") || word.equals("timeout")) {
                // Heard nothing worth a word: an empty utterance, not a failure.
                onSttEvent(mSession, 2, "", 0.0f);
            }
            else {
                onSttEvent(mSession, 3, word, 0.0f);
            }
            finishSession(mSession);
        }
    }
}
