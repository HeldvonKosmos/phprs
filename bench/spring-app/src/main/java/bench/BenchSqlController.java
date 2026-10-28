package bench;

import tools.jackson.databind.JsonNode;
import java.security.SecureRandom;
import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import org.springframework.http.ResponseEntity;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.security.crypto.bcrypt.BCryptPasswordEncoder;
import org.springframework.web.bind.annotation.*;

/**
 * Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
 * SQLite; the same statements and answers as the Laravel app's
 * BenchSqlController.
 */
@RestController
public class BenchSqlController {
    record Message(String message) {}
    record Status(String status) {}
    record Token(String token) {}
    record Note(long id, String title, String body, boolean done, int priority, String created_at, String updated_at) {}
    record NoteData(Note data) {}
    record Page(List<Note> data, long total, int per_page, int current_page) {}
    record Sqlite(String version, List<String> compile_options, Map<String, Object> runtime) {}

    static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss");
    private static final SecureRandom RANDOM = new SecureRandom();
    // strength 10 as on every side (Spring's default, set explicitly)
    private static final BCryptPasswordEncoder BCRYPT = new BCryptPasswordEncoder(10);

    private final JdbcTemplate db;

    public BenchSqlController(JdbcTemplate db) {
        this.db = db;
    }

    static String now() {
        return ZonedDateTime.now(ZoneOffset.UTC).format(TIME);
    }

    // PHP's (int) cast of a query value: leading digits, else 0
    static int asInt(String value, int fallback) {
        String s = (value == null ? String.valueOf(fallback) : value).strip();
        var m = java.util.regex.Pattern.compile("^[+-]?\\d+").matcher(s);
        if (!m.find()) return 0;
        try {
            return Integer.parseInt(m.group());
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    static Note note(Map<String, Object> r) {
        return new Note(((Number) r.get("id")).longValue(), (String) r.get("title"), (String) r.get("body"), ((Number) r.get("done")).intValue() == 1, ((Number) r.get("priority")).intValue(), (String) r.get("created_at"), (String) r.get("updated_at"));
    }

    @GetMapping({"/health", "/api/bench/sql/health"})
    public Status health() {
        return new Status("ok");
    }

    /** The SQLite this side runs and the JVM settings, for the header. */
    @GetMapping("/api/bench/sql/sqlite")
    public Sqlite sqlite() {
        var rt = Runtime.getRuntime();
        var runtime = new java.util.LinkedHashMap<String, Object>();
        runtime.put("java", System.getProperty("java.version"));
        runtime.put("aot_cache", java.lang.management.ManagementFactory.getRuntimeMXBean().getInputArguments().stream().anyMatch(x -> x.startsWith("-XX:AOTCache=")));
        runtime.put("spring_boot", org.springframework.boot.SpringBootVersion.getVersion());
        runtime.put("available_processors", rt.availableProcessors());
        runtime.put("max_heap_mib", rt.maxMemory() / (1024 * 1024));
        runtime.put("gc", java.lang.management.ManagementFactory.getGarbageCollectorMXBeans().stream().map(java.lang.management.GarbageCollectorMXBean::getName).toList());
        runtime.put("virtual_threads", Boolean.getBoolean("spring.threads.virtual.enabled") || "true".equals(System.getenv("SPRING_THREADS_VIRTUAL_ENABLED")));
        return new Sqlite(db.queryForObject("select sqlite_version()", String.class), db.queryForList("pragma compile_options", String.class), runtime);
    }

    @PostMapping("/api/bench/sql/login")
    public ResponseEntity<Object> login(@RequestBody(required = false) JsonNode body) {
        JsonNode email = body == null ? null : body.get("email"), password = body == null ? null : body.get("password");
        if (email == null || !email.isString() || password == null || !password.isString()) return ResponseEntity.status(422).body(new Message("The email and password fields are required."));
        var rows = db.queryForList("SELECT id, password FROM users WHERE email = ? LIMIT 1", email.asString());
        if (rows.isEmpty() || !BCRYPT.matches(password.asString(), (String) rows.get(0).get("password"))) return ResponseEntity.status(401).body(new Message("Invalid credentials."));
        byte[] raw = new byte[32];
        RANDOM.nextBytes(raw);
        String token = HexFormat.of().formatHex(raw);
        db.update("INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)", rows.get(0).get("id"), BenchToken.sha256(token), System.currentTimeMillis() / 1000, now());
        return ResponseEntity.ok(new Token(token));
    }

    /** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
    @GetMapping("/api/bench/sql/notes")
    public Page index(@RequestAttribute("benchUser") BenchToken.User user, @RequestParam(required = false) String done, @RequestParam(name = "per_page", required = false) String perPageText, @RequestParam(required = false) String page) {
        int d = "true".equals(done == null ? "false" : done) ? 1 : 0;
        int perPage = asInt(perPageText, 15), p = asInt(page, 1);
        long total = ((Number) db.queryForList("SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", user.id(), d).get(0).get("total")).longValue();
        var rows = db.queryForList("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", user.id(), d, perPage, (p - 1) * perPage);
        return new Page(rows.stream().map(BenchSqlController::note).toList(), total, perPage, p);
    }

    @GetMapping("/api/bench/sql/notes/{id}")
    public ResponseEntity<Object> show(@RequestAttribute("benchUser") BenchToken.User user, @PathVariable String id) {
        var rows = db.queryForList("SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", asInt(id, 0), user.id());
        if (rows.isEmpty()) return ResponseEntity.status(404).body(new Message("Not found."));
        return ResponseEntity.ok(new NoteData(note(rows.get(0))));
    }

    @PostMapping("/api/bench/sql/notes")
    public ResponseEntity<Object> store(@RequestAttribute("benchUser") BenchToken.User user, @RequestBody(required = false) JsonNode body) {
        JsonNode title = body == null ? null : body.get("title");
        if (title == null || !title.isString() || title.asString().isEmpty()) return ResponseEntity.status(422).body(new Message("The title field is required."));
        JsonNode b = body.get("body"), pr = body.get("priority");
        String text = b != null && b.isString() ? b.asString() : null;
        int priority = pr == null ? 3 : pr.isInt() ? pr.asInt() : 3;
        String t = now();
        long id = ((Number) db.queryForList("INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", user.id(), title.asString(), text, priority, t, t).get(0).get("id")).longValue();
        return ResponseEntity.status(201).body(new NoteData(new Note(id, title.asString(), text, false, priority, t, t)));
    }
}
