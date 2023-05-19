package dev.makepad.android;

public class HttpResponse {
    private int statusCode;
    private Map<String, List<String>> headers;
    private byte[] body;

    public HttpResponse(int statusCode, Map<String, List<String>> headers, byte[] body) {
        this.statusCode = statusCode;
        this.headers = headers;
        this.body = body;
    }

    public int getStatusCode() {
        return statusCode;
    }

    public Map<String, List<String>> getHeaders() {
        return headers;
    }

    public byte[] getBody() {
        return body;
    }
}
