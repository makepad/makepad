package dev.makepad.android;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.content.pm.PackageManager;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;

/**
 * A read-only provider that hands other apps `content://` URIs for files in chosen
 * subdirectories of the app's cache directory, like androidx's FileProvider but without
 * the library. An app opts in by declaring it in its `AndroidManifest.xml.template`:
 *
 * <pre>
 * &lt;provider
 *     android:name="dev.makepad.android.MakepadFileProvider"
 *     android:authorities="{package_id}.robius.share"
 *     android:exported="false"
 *     android:grantUriPermissions="true"&gt;
 *     &lt;meta-data
 *         android:name="dev.makepad.android.MakepadFileProvider.cacheDirs"
 *         android:value="robius-share" /&gt;
 * &lt;/provider&gt;
 * </pre>
 *
 * `cacheDirs` is a comma-separated list of subdirectory names. The file
 * `<cacheDir>/<dir>/<path>` is served as `content://<authority>/<dir>/<path>`, and only to
 * apps that were granted that URI (e.g., with `Intent.FLAG_GRANT_READ_URI_PERMISSION`).
 */
public class MakepadFileProvider extends ContentProvider {
    private static final String CACHE_DIRS_META_DATA =
            "dev.makepad.android.MakepadFileProvider.cacheDirs";
    private static final String[] DEFAULT_PROJECTION = {
            OpenableColumns.DISPLAY_NAME,
            OpenableColumns.SIZE,
    };

    private final HashSet<String> cacheDirs = new HashSet<>();

    @Override
    public boolean onCreate() {
        return true;
    }

    // `resolveContentProvider(String, int)` is deprecated in API 33, but its replacement isn't
    // available before that.
    @SuppressWarnings("deprecation")
    @Override
    public void attachInfo(Context context, ProviderInfo info) {
        super.attachInfo(context, info);
        // As with FileProvider: exported, any app could read these files without a grant.
        if (info.exported) {
            throw new SecurityException("MakepadFileProvider must not be exported");
        }
        if (!info.grantUriPermissions) {
            throw new SecurityException("MakepadFileProvider must grant URI permissions");
        }

        // The ProviderInfo given here usually lacks meta-data, so look it up again.
        Bundle metaData = info.metaData;
        if (metaData == null) {
            ProviderInfo withMetaData = context.getPackageManager().resolveContentProvider(
                    info.authority.split(";")[0],
                    PackageManager.GET_META_DATA);
            metaData = withMetaData == null ? null : withMetaData.metaData;
        }
        String dirs = metaData == null ? null : metaData.getString(CACHE_DIRS_META_DATA);
        if (dirs == null) {
            return;
        }
        for (String dir : dirs.split(",")) {
            dir = dir.trim();
            if (!dir.isEmpty() && !dir.equals(".") && !dir.equals("..") && dir.indexOf('/') < 0) {
                cacheDirs.add(dir);
            }
        }
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) {
            throw new SecurityException("MakepadFileProvider only opens files for reading");
        }
        return ParcelFileDescriptor.open(fileForUri(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(
            Uri uri,
            String[] projection,
            String selection,
            String[] selectionArgs,
            String sortOrder) {
        File file;
        try {
            file = fileForUri(uri);
        } catch (FileNotFoundException e) {
            return null;
        }

        // Receivers ask for the name and size of a shared file; nothing else is known.
        ArrayList<String> columns = new ArrayList<>();
        ArrayList<Object> values = new ArrayList<>();
        for (String column : projection != null ? projection : DEFAULT_PROJECTION) {
            if (OpenableColumns.DISPLAY_NAME.equals(column)) {
                columns.add(column);
                values.add(file.getName());
            } else if (OpenableColumns.SIZE.equals(column)) {
                columns.add(column);
                values.add(file.length());
            }
        }
        MatrixCursor cursor = new MatrixCursor(columns.toArray(new String[0]), 1);
        cursor.addRow(values.toArray());
        return cursor;
    }

    @Override
    public String getType(Uri uri) {
        String name = uri.getLastPathSegment();
        int dot = name == null ? -1 : name.lastIndexOf('.');
        if (dot >= 0) {
            String mimeType = MimeTypeMap.getSingleton().getMimeTypeFromExtension(
                    name.substring(dot + 1).toLowerCase(Locale.ROOT));
            if (mimeType != null) {
                return mimeType;
            }
        }
        return "application/octet-stream";
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("MakepadFileProvider is read-only");
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("MakepadFileProvider is read-only");
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("MakepadFileProvider is read-only");
    }

    private File fileForUri(Uri uri) throws FileNotFoundException {
        List<String> segments = uri.getPathSegments();
        if (segments.size() < 2 || !cacheDirs.contains(segments.get(0))) {
            throw new FileNotFoundException("Not a shared file: " + uri);
        }
        try {
            File root = new File(getContext().getCacheDir(), segments.get(0)).getCanonicalFile();
            File file = root;
            for (String segment : segments.subList(1, segments.size())) {
                file = new File(file, segment);
            }
            // Resolving `..` and symlinks must not lead out of the shared directory.
            file = file.getCanonicalFile();
            if (!file.getPath().startsWith(root.getPath() + File.separator) || !file.isFile()) {
                throw new FileNotFoundException("Not a shared file: " + uri);
            }
            return file;
        } catch (IOException e) {
            throw new FileNotFoundException("Not a shared file: " + uri);
        }
    }
}
