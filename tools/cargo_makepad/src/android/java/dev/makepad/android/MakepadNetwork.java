package dev.makepad.android;

import android.os.AsyncTask;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.Map;
import java.util.List;
import java.nio.charset.StandardCharsets;

public class MakepadNetwork {

    public MakepadNetwork() {} // TODO: this might just be a static class.

    public CompletableFuture<HttpResponse> performNetworkRequest(String url, String method, String headers, byte[] body) {
        return CompletableFuture.supplyAsync(() -> {
            HttpURLConnection connection = null;
            HttpResponse response = null;

            try {
                URL urlObj = new URL(url);
                connection = (HttpURLConnection) urlObj.openConnection();
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
