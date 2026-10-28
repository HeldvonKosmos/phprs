package bench;

import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Map;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Component;
import org.springframework.web.servlet.HandlerInterceptor;

/**
 * Measurement tool (BENCH3): the token check of the framework-core
 * benchmark, the same statements as in the Laravel app; the token's use is
 * written at most once a second, as Sanctum does in effect.
 */
@Component
public class BenchToken implements HandlerInterceptor {
    public record User(long id, String name, String email) {}

    private final JdbcTemplate db;

    public BenchToken(JdbcTemplate db) {
        this.db = db;
    }

    static String sha256(String s) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(s.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    @Override
    public boolean preHandle(HttpServletRequest request, HttpServletResponse response, Object handler) throws Exception {
        String header = request.getHeader("Authorization");
        String token = header != null && header.startsWith("Bearer ") ? header.substring(7) : "";
        if (token.isEmpty()) return unauthenticated(response);
        var rows = db.queryForList("SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", sha256(token));
        if (rows.isEmpty()) return unauthenticated(response);
        Map<String, Object> row = rows.get(0);
        long now = System.currentTimeMillis() / 1000;
        if (now - ((Number) row.get("last_used_at")).longValue() >= 1) db.update("UPDATE tokens SET last_used_at = ? WHERE id = ?", now, row.get("token_id"));
        request.setAttribute("benchUser", new User(((Number) row.get("id")).longValue(), (String) row.get("name"), (String) row.get("email")));
        return true;
    }

    private static boolean unauthenticated(HttpServletResponse response) throws java.io.IOException {
        response.setStatus(401);
        response.setContentType("application/json");
        response.getWriter().write("{\"message\":\"Unauthenticated.\"}");
        return false;
    }
}
