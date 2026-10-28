package bench;

import jakarta.validation.ConstraintViolation;
import jakarta.validation.Validator;
import jakarta.validation.constraints.Max;
import jakarta.validation.constraints.Min;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Size;
import java.util.*;
import java.util.stream.Collectors;
import org.springframework.http.ResponseEntity;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.web.bind.annotation.*;
import tools.jackson.databind.JsonNode;

/**
 * Measurement tool (BENCH3, THESIS1): workloads with an adjustable amount of
 * application logic per request (`n`), the same steps and answers as the
 * Laravel app's BenchLogicController.
 */
@RestController
@RequestMapping("/api/bench/logic")
public class BenchLogicController {
    record UserView(long id, String name, String email) {}

    /** A note hydrated from a raw row, with its user (THESIS1). */
    record NoteView(long id, String title, String body, boolean done, int priority, String created_at, UserView user) {
        static NoteView fromRow(Map<String, Object> r) {
            return new NoteView(((Number) r.get("id")).longValue(), (String) r.get("title"), (String) r.get("body"), ((Number) r.get("done")).intValue() == 1, ((Number) r.get("priority")).intValue(), (String) r.get("created_at"),
                    new UserView(((Number) r.get("user_id")).longValue(), (String) r.get("user_name"), (String) r.get("user_email")));
        }
    }

    record Group(int count, long chars, String first) {}
    record Flags(boolean x, boolean y) {}
    record Meta(String created, double score, Flags flags) {}
    record JsonItem(int id, String name, List<String> tags, Meta meta) {}

    /** One element of the validate workload: Bean Validation constraints, the rules of the Laravel app. */
    record Item(@NotNull @Size(min = 1, max = 120) String title, @NotNull @Min(1) @Max(5) Integer priority, List<@Size(max = 20) String> tags) {}

    private final JdbcTemplate db;
    private final Validator validator;

    public BenchLogicController(JdbcTemplate db, Validator validator) {
        this.db = db;
        this.validator = validator;
    }

    private static int n(String value) {
        return Math.min(1000, Math.max(0, BenchSqlController.asInt(value, 1)));
    }

    private List<Map<String, Object>> rows(BenchToken.User user, int n) {
        return db.queryForList("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", user.id(), n);
    }

    /** n notes with their user, hydrated into objects and listed. */
    @GetMapping("/hydrate")
    public Map<String, Object> hydrate(@RequestAttribute("benchUser") BenchToken.User user, @RequestParam(name = "n", required = false) String n) {
        return Map.of("data", rows(user, n(n)).stream().map(NoteView::fromRow).toList());
    }

    /** n notes prepared with streams: map, filter, groupingBy, sorted, sum. */
    @GetMapping("/collect")
    public Map<String, Object> collect(@RequestAttribute("benchUser") BenchToken.User user, @RequestParam(name = "n", required = false) String n) {
        record Row(String title, int priority, boolean done, int chars) {}
        Map<Integer, List<Row>> groups = rows(user, n(n)).stream()
                .map(r -> { String t = (String) r.get("title"), b = (String) r.get("body"); return new Row(t, ((Number) r.get("priority")).intValue(), ((Number) r.get("done")).intValue() == 1, t.length() + (b == null ? 0 : b.length())); })
                .filter(r -> !r.done())
                .collect(Collectors.groupingBy(Row::priority, TreeMap::new, Collectors.toList()));
        Map<String, Group> out = new LinkedHashMap<>();
        groups.forEach((k, g) -> out.put(String.valueOf(k), new Group(g.size(), g.stream().mapToLong(Row::chars).sum(), g.stream().sorted(Comparator.comparing(Row::title)).findFirst().orElseThrow().title())));
        return Map.of("groups", out);
    }

    private static final List<String> ORDER = List.of("items", "title", "priority", "tags", "tag");

    /**
     * A nested input of n items, validated; answers the number of items. The
     * JSON types are checked first (Jackson would turn 7 into "7" for a
     * String), the value constraints with Bean Validation; errors in
     * Laravel's form.
     */
    @PostMapping("/validate")
    public ResponseEntity<Object> validate(@RequestBody(required = false) JsonNode body) {
        record Found(int rank, List<Integer> idx, String key, String message) {}
        List<Found> found = new ArrayList<>();
        JsonNode items = body == null ? null : body.get("items");
        if (items == null || items.isNull() || (items.isArray() && items.isEmpty())) found.add(new Found(0, List.of(), "items", "The items field is required."));
        else if (!items.isArray()) found.add(new Found(0, List.of(), "items", "The items field must be an array."));
        else {
            int i = 0;
            for (JsonNode it : items.values()) {
                String p = "items." + i;
                JsonNode title = it.get("title"), priority = it.get("priority"), tags = it.get("tags");
                String t = null;
                Integer pr = null;
                List<String> tg = null;
                Set<String> typeErrors = new HashSet<>();
                if (title != null && !title.isNull()) {
                    if (title.isString()) t = title.asString();
                    else { found.add(new Found(1, List.of(i), p + ".title", "The " + p + ".title field must be a string.")); typeErrors.add("title"); }
                }
                if (priority != null && !priority.isNull()) {
                    if (priority.isIntegralNumber() && priority.canConvertToInt()) pr = priority.asInt();
                    else if (priority.isString() && priority.asString().matches("^[+-]?\\d+$")) pr = Integer.parseInt(priority.asString());
                    else { found.add(new Found(2, List.of(i), p + ".priority", "The " + p + ".priority field must be an integer.")); typeErrors.add("priority"); }
                }
                if (tags != null && !tags.isNull()) {
                    if (!tags.isArray()) { found.add(new Found(3, List.of(i), p + ".tags", "The " + p + ".tags field must be an array.")); typeErrors.add("tags"); }
                    else {
                        tg = new ArrayList<>();
                        int j = 0;
                        for (JsonNode tag : tags.values()) {
                            if (tag.isString()) tg.add(tag.asString());
                            else { found.add(new Found(4, List.of(i, j), p + ".tags." + j, "The " + p + ".tags." + j + " field must be a string.")); tg.add(null); }
                            j++;
                        }
                    }
                }
                for (ConstraintViolation<Item> v : validator.validate(new Item(t, pr, tg))) {
                    String path = v.getPropertyPath().toString();
                    String field = path.replaceAll("\\[.*", "");
                    if (typeErrors.contains(field)) continue;
                    String annotation = v.getConstraintDescriptor().getAnnotation().annotationType().getSimpleName();
                    if (field.equals("tags")) {
                        var m = java.util.regex.Pattern.compile("\\[(\\d+)\\]").matcher(path);
                        if (!m.find()) continue;
                        int j = Integer.parseInt(m.group(1));
                        if (tg.get(j) == null) continue;
                        found.add(new Found(4, List.of(i, j), p + ".tags." + j, "The " + p + ".tags." + j + " field must not be greater than 20 characters."));
                    } else if (field.equals("title")) {
                        String key = p + ".title";
                        found.add(new Found(1, List.of(i), key, annotation.equals("Size") && t != null && !t.isEmpty() ? "The " + key + " field must not be greater than 120 characters." : "The " + key + " field is required."));
                    } else if (field.equals("priority")) {
                        String key = p + ".priority";
                        found.add(new Found(2, List.of(i), key, annotation.equals("NotNull") ? "The " + key + " field is required." : "The " + key + " field must be between 1 and 5."));
                    }
                }
                i++;
            }
        }
        if (found.isEmpty()) return ResponseEntity.ok(Map.of("valid", items.size()));
        found.sort(Comparator.comparingInt(Found::rank).thenComparing((a, b) -> { for (int k = 0; k < Math.max(a.idx().size(), b.idx().size()); k++) { int x = k < a.idx().size() ? a.idx().get(k) : -1, y = k < b.idx().size() ? b.idx().get(k) : -1; if (x != y) return Integer.compare(x, y); } return 0; }));
        Map<String, List<String>> errors = new LinkedHashMap<>();
        for (Found f : found) errors.computeIfAbsent(f.key(), k -> new ArrayList<>()).add(f.message());
        int more = found.size() - 1;
        String message = found.get(0).message() + (more > 0 ? " (and " + more + " more " + (more == 1 ? "error" : "errors") + ")" : "");
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("message", message);
        out.put("errors", errors);
        return ResponseEntity.status(422).body(out);
    }

    /** A nested structure of depth 3 with n items, no database. */
    @GetMapping("/json")
    public Map<String, Object> json(@RequestParam(name = "n", required = false) String n) {
        int count = n(n);
        List<JsonItem> items = new ArrayList<>(count);
        for (int i = 0; i < count; i++) items.add(new JsonItem(i, "item " + i, List.of("a", "b", "c"), new Meta("2026-10-27", i * 1.5, new Flags(true, false))));
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("count", count);
        out.put("items", items);
        return out;
    }
}
