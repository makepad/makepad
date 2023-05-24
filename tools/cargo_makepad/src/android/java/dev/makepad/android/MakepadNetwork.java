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

    public CompletableFuture<HttpResponse> performNetworkRequest(HttpRequest request) {
        return CompletableFuture.supplyAsync(() -> {
            HttpURLConnection connection = null;
            HttpResponse response = null;

            try {
                URL urlObj = new URL(request.getUrl());
                connection = (HttpURLConnection) urlObj.openConnection();
                connection.setRequestMethod(request.getMethod());

                String[] headerPairs = request.getHeaders().split(";");

                for (String headerPair : headerPairs) {
                    String[] parts = headerPair.split(":");
                    if (parts.length == 2) {
                        String key = parts[0].trim();
                        String value = parts[1].trim();
                        connection.setRequestProperty(key, value);
                    }
                }

                byte[] body = request.getBody();
                if (body != null) {
                    connection.setDoOutput(true);
                    try (OutputStream outputStream = connection.getOutputStream()) {
                        outputStream.write(body);
                    }
                }

                int statusCode = connection.getResponseCode();

                byte[] responseBody = readBytesFromStream(connection.getInputStream());

                Map<String, List<String>> responseHeaders = connection.getHeaderFields();

                response = new HttpResponse(statusCode, responseHeaders, responseBody);
            } catch (IOException e) {
                e.printStackTrace(); // TODO: handle exception
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
}
