package dev.makepad.android;

import java.io.IOException;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.URL;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.Map;
import java.util.List;
import java.nio.charset.StandardCharsets;

import android.util.Log;

class HttpResponse {
    private int statusCode;
    private String headers;
    private byte[] body;

    public HttpResponse(int statusCode, String headers, byte[] body) {
        this.statusCode = statusCode;
        this.headers = headers;
        this.body = body;
    }

    public int getStatusCode() {
        return statusCode;
    }

    public String getHeaders() {
        return headers;
    }

    public byte[] getBody() {
        return body;
    }
}

public class MakepadNetwork {

    public MakepadNetwork() {
    }

    // Build an explicit HTTP proxy from the standard JVM proxy system properties
    // (http.proxyHost/Port for http URLs, https.proxyHost/Port for https URLs).
    // Returns null when no proxy is configured, so the caller connects directly.
    private static Proxy selectProxy(URL urlObj) {
        String scheme = urlObj.getProtocol();
        if (scheme == null) {
            return null;
        }
        scheme = scheme.toLowerCase();
        if (!scheme.equals("http") && !scheme.equals("https")) {
            return null;
        }
        String host = System.getProperty(scheme + ".proxyHost");
        String portStr = System.getProperty(scheme + ".proxyPort");
        if (host == null || host.isEmpty() || portStr == null || portStr.isEmpty()) {
            return null;
        }
        try {
            int port = Integer.parseInt(portStr.trim());
            return new Proxy(Proxy.Type.HTTP, new InetSocketAddress(host, port));
        } catch (NumberFormatException e) {
            return null;
        }
    }

    public CompletableFuture<HttpResponse> performHttpRequest(String url, String method, String headers, byte[] body) {
        return CompletableFuture.supplyAsync(() -> {
            HttpURLConnection connection = null;
            HttpResponse response = null;

            try {
                URL urlObj = new URL(url);
                // Route app-initiated requests through the proxy named by the standard
                // JVM proxy system properties (Android populates them from the
                // system / Wi-Fi proxy configuration; a host may also set them itself).
                // We apply it explicitly rather than relying on the default
                // ProxySelector picking the properties up.
                Proxy proxy = selectProxy(urlObj);
                connection = (HttpURLConnection) (proxy != null
                        ? urlObj.openConnection(proxy)
                        : urlObj.openConnection());
                connection.setRequestMethod(method);

                String[] headerPairs = headers.split("\r\n");

                for (String headerPair : headerPairs) {
                    String[] parts = headerPair.split(":");
                    if (parts.length == 2) {
                        String key = parts[0].trim();
                        String value = parts[1].trim();
                        connection.setRequestProperty(key, value);
                    }
                }

                if (body != null) {
                    connection.setDoOutput(true);
                    try (OutputStream outputStream = connection.getOutputStream()) {
                        outputStream.write(body);
                    }
                }

                int statusCode = connection.getResponseCode();

                byte[] responseBody;
                if (statusCode >= 400) {
                    responseBody = readBytesFromStream(connection.getErrorStream());
                } else {
                    responseBody = readBytesFromStream(connection.getInputStream());
                }

                String responseHeaders = getHeadersAsString(connection.getHeaderFields());

                response = new HttpResponse(statusCode, responseHeaders, responseBody);
            } catch (IOException e) {
               throw(new RuntimeException(e));
            } finally {
                if (connection != null) {
                    connection.disconnect();
                }
            }

            return response;
        });
    }

    private byte[] readBytesFromStream(InputStream inputStream) throws IOException {
        ByteArrayOutputStream outputStream = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int bytesRead;
        while ((bytesRead = inputStream.read(buffer)) != -1) {
            outputStream.write(buffer, 0, bytesRead);
        }
        return outputStream.toByteArray();
    }

    private String getHeadersAsString(Map<String, List<String>> headers) {
        StringBuilder sb = new StringBuilder();
        for (Map.Entry<String, List<String>> entry : headers.entrySet()) {
            String key = entry.getKey();
            List<String> values = entry.getValue();
            for (String value : values) {
                sb.append(key).append(": ").append(value).append("\r\n");
            }
        }
        return sb.toString();
    }
}