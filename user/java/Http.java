// A small HTTP server on the JDK's com.sun.net.httpserver (NIO: a selector
// over Winsock, which on Windows is wepoll over AFD), serving one page on
// port 7 of every address to the host (the `java_http` boot scenario).
import com.sun.net.httpserver.HttpServer;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;

public class Http {
    public static void main(String[] args) throws Exception {
        byte[] page = "hello from Java on gala\n".getBytes(StandardCharsets.UTF_8);
        HttpServer server = HttpServer.create(new InetSocketAddress(7), 0);
        server.createContext("/", exchange -> {
            exchange.getResponseHeaders().set("Content-Type", "text/plain");
            exchange.sendResponseHeaders(200, page.length);
            try (OutputStream out = exchange.getResponseBody()) {
                out.write(page);
            }
            System.out.println("java_http: served " + exchange.getRequestURI());
        });
        server.start();
        System.out.println("java_http: listening on port 7");
    }
}
