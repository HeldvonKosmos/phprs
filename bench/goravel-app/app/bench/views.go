package bench

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
)

// Note is a note as the API answers it (the fields in Laravel's order).
type Note struct {
	ID        int64 `json:"id"`
	Title     any   `json:"title"`
	Body      any   `json:"body"`
	Done      bool  `json:"done"`
	Priority  int64 `json:"priority"`
	CreatedAt any   `json:"created_at"`
	UpdatedAt any   `json:"updated_at"`
}

func NoteOf(r map[string]any) Note {
	return Note{Int64(r["id"]), Str(r["title"]), Str(r["body"]), Int64(r["done"]) == 1, Int64(r["priority"]), Str(r["created_at"]), Str(r["updated_at"])}
}

// UserView is the user of a hydrated note (THESIS1).
type UserView struct {
	ID    int64
	Name  any
	Email any
}

func (u UserView) ToArray() Obj {
	return Obj{{"id", u.ID}, {"name", u.Name}, {"email", u.Email}}
}

// NoteView is a note hydrated from a raw row, with its user (THESIS1).
type NoteView struct {
	ID        int64
	Title     any
	Body      any
	Done      bool
	Priority  int64
	CreatedAt any
	User      UserView
}

func NoteViewFromRow(r map[string]any) NoteView {
	return NoteView{Int64(r["id"]), Str(r["title"]), Str(r["body"]), Int64(r["done"]) == 1, Int64(r["priority"]), Str(r["created_at"]), UserView{Int64(r["user_id"]), Str(r["user_name"]), Str(r["user_email"])}}
}

func (n NoteView) ToArray() Obj {
	return Obj{{"id", n.ID}, {"title", n.Title}, {"body", n.Body}, {"done", n.Done}, {"priority", n.Priority}, {"created_at", n.CreatedAt}, {"user", n.User.ToArray()}}
}

// KV and Obj: a JSON object that keeps its key order, as PHP arrays do.
type KV struct {
	K string
	V any
}

type Obj []KV

func (o Obj) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, kv := range o {
		if i > 0 {
			b.WriteByte(',')
		}
		k, _ := json.Marshal(kv.K)
		b.Write(k)
		b.WriteByte(':')
		v, err := json.Marshal(kv.V)
		if err != nil {
			return nil, err
		}
		b.Write(v)
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

var order = map[string]int{"items": 0, "title": 1, "priority": 2, "tags": 3, "tag": 4}

type found struct {
	rank int
	nums []int
	key  string
	msg  string
}

// LaravelErrors are the failed keys of a validation in Laravel's form:
// Laravel's wording (from the rule the key names and the value found
// there), ordered by rule and then by element, the first message with
// "(and N more errors)".
func LaravelErrors(keys []string, value func(string) (any, bool)) Obj {
	var fs []found
	for _, key := range keys {
		var parts []string
		var nums []int
		start := 0
		for i := 0; i <= len(key); i++ {
			if i == len(key) || key[i] == '.' {
				p := key[start:i]
				parts = append(parts, p)
				var n int
				if _, err := fmt.Sscanf(p, "%d", &n); err == nil && fmt.Sprint(n) == p {
					nums = append(nums, n)
				}
				start = i + 1
			}
		}
		last := parts[len(parts)-1]
		rank := last
		if len(parts) == 1 {
			rank = "items"
		} else if len(nums) > 0 && fmt.Sprint(nums[len(nums)-1]) == last {
			rank = "tag"
		}
		v, present := value(key)
		missing := !present || v == nil
		var msg string
		switch rank {
		case "items":
			if l, ok := v.([]any); missing || (ok && len(l) == 0) {
				msg = "The " + key + " field is required."
			} else {
				msg = "The " + key + " field must be an array."
			}
		case "title":
			if s, ok := v.(string); missing || (ok && s == "") {
				msg = "The " + key + " field is required."
			} else if !ok {
				msg = "The " + key + " field must be a string."
			} else {
				msg = "The " + key + " field must not be greater than 120 characters."
			}
		case "priority":
			if f, ok := v.(float64); missing {
				msg = "The " + key + " field is required."
			} else if !ok || f != float64(int64(f)) {
				msg = "The " + key + " field must be an integer."
			} else {
				msg = "The " + key + " field must be between 1 and 5."
			}
		case "tags":
			msg = "The " + key + " field must be an array."
		default:
			if _, ok := v.(string); !ok {
				msg = "The " + key + " field must be a string."
			} else {
				msg = "The " + key + " field must not be greater than 20 characters."
			}
		}
		fs = append(fs, found{order[rank], nums, key, msg})
	}
	sort.SliceStable(fs, func(i, j int) bool {
		if fs[i].rank != fs[j].rank {
			return fs[i].rank < fs[j].rank
		}
		for k := range fs[i].nums {
			if k >= len(fs[j].nums) {
				return false
			}
			if fs[i].nums[k] != fs[j].nums[k] {
				return fs[i].nums[k] < fs[j].nums[k]
			}
		}
		return len(fs[i].nums) < len(fs[j].nums)
	})
	errs := Obj{}
	for _, f := range fs {
		errs = append(errs, KV{f.key, []string{f.msg}})
	}
	message := fs[0].msg
	if more := len(fs) - 1; more == 1 {
		message += " (and 1 more error)"
	} else if more > 1 {
		message += fmt.Sprintf(" (and %d more errors)", more)
	}
	return Obj{{"message", message}, {"errors", errs}}
}
