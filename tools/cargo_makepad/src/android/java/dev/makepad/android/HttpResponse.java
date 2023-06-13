package dev.makepad.android;
import java.util.Map;
import java.util.List;

public class HttpResponse {
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
