package dev.makepad.android;

public class HttpRequest {
    private String url;
    private String method;
    private Map<String, List<String>> headers;
    private byte[] body;

    public int getUrl() {
        return url;
    }

    public int getMethod() {
        return method;
    }

    public Map<String, List<String>> getHeaders() {
        return headers;
    }

    public byte[] getBody() {
        return body;
    }
}
