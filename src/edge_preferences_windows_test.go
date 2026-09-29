//go:build windows

package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestPrepareEdgeProfile(t *testing.T) {
	for _, existing := range []string{"", `{"translate":{"enabled":true,"other":"keep"},"large":9007199254740993,"session":{"restore_on_startup":1}}`} {
		t.Run(existing, func(t *testing.T) {
			profile := filepath.Join(t.TempDir(), "Profile")
			path := filepath.Join(profile, "Default", "Preferences")
			if existing != "" {
				if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(path, []byte(existing), 0600); err != nil {
					t.Fatal(err)
				}
			}
			for i := 0; i < 2; i++ {
				if err := prepareEdgeProfile(profile); err != nil {
					t.Fatal(err)
				}
			}
			data, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			var result map[string]json.RawMessage
			if err := json.Unmarshal(data, &result); err != nil {
				t.Fatal(err)
			}
			var translate map[string]json.RawMessage
			if err := json.Unmarshal(result["translate"], &translate); err != nil || string(translate["enabled"]) != "false" {
				t.Fatalf("translation offer is enabled: %s", data)
			}
			if existing != "" && (string(result["large"]) != "9007199254740993" || string(result["session"]) != `{"restore_on_startup":1}` || string(translate["other"]) != `"keep"`) {
				t.Fatalf("unrelated preferences changed: %s", data)
			}
		})
	}
}

func TestPrepareEdgeProfilePreservesInvalidPreferences(t *testing.T) {
	for _, existing := range []string{`{broken`, `null`, `{"translate":null}`, `{"translate":true}`} {
		profile := t.TempDir()
		path := filepath.Join(profile, "Default", "Preferences")
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(existing), 0600); err != nil {
			t.Fatal(err)
		}
		if err := prepareEdgeProfile(profile); err == nil {
			t.Fatal("invalid preferences accepted")
		}
		data, err := os.ReadFile(path)
		if err != nil || string(data) != existing {
			t.Fatal("invalid preferences were overwritten")
		}
	}
}
