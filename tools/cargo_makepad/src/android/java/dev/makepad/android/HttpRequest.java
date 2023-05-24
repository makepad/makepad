package dev.makepad.android;

public class HttpRequest {
    private String url;
    private String method;
    private String headers;
    private byte[] body;

    public String getUrl() {
        return url;
    }

    public String getMethod() {
        return method;
    }

    public String getHeaders() {
        return headers;
    }

    public byte[] getBody() {
        return body;
    }
}
