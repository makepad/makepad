package dev.makepad.android;

import android.os.AsyncTask;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

public class MakepadNetwork {

    public MakepadNetwork() {} // TODO: this might just be a static class.

    public CompletableFuture<HttpResponse> performNetworkRequest(HttpRequest request) {
        return CompletableFuture.supplyAsync(() -> {
            HttpURLConnection connection = null;
            HttpResponse response = null;

            try {
                URL urlObj = new URL(request.get_url());
                connection = (HttpURLConnection) urlObj.openConnection();
                connection.setRequestMethod(request.get_method());


                headers = request.get_headers();
                if (headers != null) {
                    for (Map.Entry<String, String> entry : headers.entrySet()) {
                        connection.setRequestProperty(entry.getKey(), entry.getValue());
                    }
                }

                body = request.get_body();
                if (body != null && !body.isEmpty()) {
                    connection.setDoOutput(true);
                    try (OutputStream outputStream = connection.getOutputStream()) {
                        outputStream.write(body.getBytes(StandardCharsets.UTF_8));
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
