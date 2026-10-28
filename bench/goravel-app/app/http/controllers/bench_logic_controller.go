package controllers

import (
	"sort"
	"strconv"
	"strings"

	"github.com/goravel/framework/contracts/http"

	"goravel/app/bench"
	"goravel/app/facades"
)

// BenchLogicController holds the workloads with an adjustable amount of
// logic per request (THESIS1, ?n=), the same in every app.
type BenchLogicController struct{}

func NewBenchLogicController() *BenchLogicController {
	return &BenchLogicController{}
}

func logicN(ctx http.Context) int {
	n := bench.AsInt(ctx.Request().Query("n", "1"), 1)
	return min(1000, max(0, n))
}

func logicRows(ctx http.Context) ([]map[string]any, error) {
	user := bench.CurrentUser(ctx.Value("benchUser"))
	return bench.Rows("SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", user.ID, logicN(ctx))
}

// Hydrate lists n notes with their user, hydrated into objects.
func (r *BenchLogicController) Hydrate(ctx http.Context) http.Response {
	rows, err := logicRows(ctx)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	data := make([]bench.Obj, 0, len(rows))
	for _, row := range rows {
		data = append(data, bench.NoteViewFromRow(row).ToArray())
	}
	return ctx.Response().Json(http.StatusOK, http.Json{"data": data})
}

// Collect prepares n notes with the language's slices and maps: map,
// filter, group by, sort by, sum.
func (r *BenchLogicController) Collect(ctx http.Context) http.Response {
	rows, err := logicRows(ctx)
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	type item struct {
		title    string
		priority int64
		chars    int
	}
	groups := map[int64][]item{}
	for _, row := range rows {
		if bench.Int64(row["done"]) == 1 {
			continue
		}
		title, _ := row["title"].(string)
		body, _ := row["body"].(string)
		p := bench.Int64(row["priority"])
		groups[p] = append(groups[p], item{title, p, len([]rune(title)) + len([]rune(body))})
	}
	keys := make([]int64, 0, len(groups))
	for k := range groups {
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool { return keys[i] < keys[j] })
	out := bench.Obj{}
	for _, k := range keys {
		g := groups[k]
		chars := 0
		first := g[0].title
		for _, it := range g {
			chars += it.chars
			if it.title < first {
				first = it.title
			}
		}
		out = append(out, bench.KV{K: strconv.FormatInt(k, 10), V: bench.Obj{{K: "count", V: len(g)}, {K: "chars", V: chars}, {K: "first", V: first}}})
	}
	return ctx.Response().Json(http.StatusOK, http.Json{"groups": out})
}

// Validate validates a nested input of n items with Goravel's validation
// (Laravel's rules); answers the number of items.
func (r *BenchLogicController) Validate(ctx http.Context) http.Response {
	data := ctx.Request().All()
	validator, err := facades.Validation().Make(ctx, data, itemRules(data))
	if err != nil {
		return ctx.Response().Json(http.StatusInternalServerError, http.Json{"message": err.Error()})
	}
	if validator.Fails() {
		keys := make([]string, 0)
		for k := range validator.Errors().All() {
			keys = append(keys, k)
		}
		return ctx.Response().Json(http.StatusUnprocessableEntity, bench.LaravelErrors(keys, func(key string) (any, bool) { return lookup(data, key) }))
	}
	items, _ := data["items"].([]any)
	return ctx.Response().Json(http.StatusOK, http.Json{"valid": len(items)})
}

// itemRules are Laravel's rules of the workload (items: required|array;
// items.*.title: required|string|max:120; items.*.priority:
// required|integer|between:1,5; items.*.tags: array; items.*.tags.*:
// string|max:20) expanded per element, as Laravel expands its wildcards:
// Goravel's wildcard rules do not report a field missing from an element
// as required, and report the wildcard key itself when the list is missing.
func itemRules(data map[string]any) map[string]any {
	rules := map[string]any{"items": "required|array"}
	items, _ := data["items"].([]any)
	for i, it := range items {
		p := "items." + strconv.Itoa(i)
		rules[p+".title"] = "required|string|max:120"
		rules[p+".priority"] = "required|integer|between:1,5"
		rules[p+".tags"] = "array"
		if m, ok := it.(map[string]any); ok {
			if tags, ok := m["tags"].([]any); ok {
				for j := range tags {
					rules[p+".tags."+strconv.Itoa(j)] = "string|max:20"
				}
			}
		}
	}
	return rules
}

// lookup finds the value at a dotted key (items.0.title) in the input.
func lookup(data map[string]any, key string) (any, bool) {
	var cur any = data
	for _, p := range strings.Split(key, ".") {
		switch c := cur.(type) {
		case map[string]any:
			v, ok := c[p]
			if !ok {
				return nil, false
			}
			cur = v
		case []any:
			i, err := strconv.Atoi(p)
			if err != nil || i < 0 || i >= len(c) {
				return nil, false
			}
			cur = c[i]
		default:
			return nil, false
		}
	}
	return cur, true
}

// Json builds a nested structure of depth 3 with n items, no database.
func (r *BenchLogicController) Json(ctx http.Context) http.Response {
	n := logicN(ctx)
	items := make([]bench.Obj, 0, n)
	for i := 0; i < n; i++ {
		items = append(items, bench.Obj{{K: "id", V: i}, {K: "name", V: "item " + strconv.Itoa(i)}, {K: "tags", V: []string{"a", "b", "c"}},
			{K: "meta", V: bench.Obj{{K: "created", V: "2026-10-27"}, {K: "score", V: float64(i) * 1.5}, {K: "flags", V: bench.Obj{{K: "x", V: true}, {K: "y", V: false}}}}}})
	}
	return ctx.Response().Json(http.StatusOK, bench.Obj{{K: "count", V: n}, {K: "items", V: items}})
}
