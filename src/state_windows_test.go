//go:build windows

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

func TestSettingsDPAPIRoundTripAndValidation(t *testing.T) {
	root := t.TempDir()
	if err := saveSettings(root, "key-id", "secret-api-token", defaultTTL, true); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(settingsPath(root))
	if bytes.Contains(raw, []byte("secret-api-token")) {
		t.Fatal("plaintext token on disk")
	}
	s, token, err := loadSettings(root)
	if err != nil || token != "secret-api-token" || !s.CacheCredentials {
		t.Fatalf("settings=%+v err=%v", s, err)
	}
	for _, tc := range []struct {
		key string
		ttl int
	}{{"../bad", 86400}, {"key-id", -1}, {"key-id", maxTTL + 1}} {
		s.TurnKeyID, s.TTL = tc.key, tc.ttl
		modified, _ := json.Marshal(s)
		if err := atomicWriteFile(settingsPath(root), modified); err != nil {
			t.Fatal(err)
		}
		if _, _, err := loadSettings(root); err == nil {
			t.Fatalf("accepted invalid settings: %+v", tc)
		}
	}
}

func TestCredentialCache(t *testing.T) {
	root := t.TempDir()
	s := Settings{TurnKeyID: "key-id", TTL: defaultTTL, CacheCredentials: true}
	token := "secret-api-token"
	servers := json.RawMessage(`[{"urls":["turn:example.com:3478"],"username":"user","credential":"turn-password"}]`)
	now := time.Now().UTC()
	if err := saveCredentialCache(root, s, token, servers, now); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(cachePath(root))
	if bytes.Contains(raw, []byte("turn-password")) || bytes.Contains(raw, []byte(token)) {
		t.Fatal("plaintext credentials on disk")
	}
	if _, err := loadCredentialCache(root, s, token, now.Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	// A reusable cache must allow startup without any network request.
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := getIceServers(ctx, root, s, token); err != nil {
		t.Fatalf("cache hit contacted network: %v", err)
	}
	for _, when := range []time.Time{now.Add(-time.Second), now.Add(19 * time.Hour), now.Add(25 * time.Hour)} {
		if _, err := loadCredentialCache(root, s, token, when); err == nil {
			t.Fatalf("accepted cache at %v", when)
		}
	}
	if _, err := loadCredentialCache(root, s, "changed-token", now); err == nil {
		t.Fatal("accepted changed token")
	}
	changed := s
	changed.TurnKeyID = "other-key"
	if _, err := loadCredentialCache(root, changed, token, now); err == nil {
		t.Fatal("accepted changed key")
	}
	changed = s
	changed.TTL = 172800
	if _, err := loadCredentialCache(root, changed, token, now); err == nil {
		t.Fatal("accepted changed TTL")
	}
	s.CacheCredentials = false
	if _, err := getIceServers(ctx, root, s, token); err == nil {
		t.Fatal("disabled cache was reused")
	}
	if err := atomicWriteFile(cachePath(root), []byte(`{"encryptedCredentials":"broken"}`)); err != nil {
		t.Fatal(err)
	}
	if _, err := loadCredentialCache(root, s, token, now); err == nil {
		t.Fatal("accepted corrupt cache")
	}
}

func TestAtomicWritePreservesLockedDestination(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(root, "state.json")
	if err := atomicWriteFile(target, []byte("old")); err != nil {
		t.Fatal(err)
	}
	path, _ := syscall.UTF16PtrFromString(target)
	handle, err := syscall.CreateFile(path, syscall.GENERIC_READ, syscall.FILE_SHARE_READ, nil, syscall.OPEN_EXISTING, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := atomicWriteFile(target, []byte("new")); err == nil {
		syscall.CloseHandle(handle)
		t.Fatal("expected locked replacement to fail")
	}
	syscall.CloseHandle(handle)
	raw, _ := os.ReadFile(target)
	if string(raw) != "old" {
		t.Fatal("old destination was lost")
	}
	if err := atomicWriteFile(target, []byte("new")); err != nil {
		t.Fatal(err)
	}
	entries, _ := os.ReadDir(root)
	if len(entries) != 1 {
		t.Fatal("temporary files leaked")
	}
}

func TestProfileAndLauncherLocks(t *testing.T) {
	root := t.TempDir()
	unlock, err := acquireLauncherLock(root)
	if err != nil {
		t.Fatal(err)
	}
	if second, err := acquireLauncherLock(root); err == nil {
		second()
		unlock()
		t.Fatal("parallel launcher accepted")
	}
	unlock()
	unlock, err = acquireLauncherLock(root)
	if err != nil {
		t.Fatal(err)
	}
	unlock()
	if err := ensureProfileStopped(root); err != nil {
		t.Fatal(err)
	}
	path, _ := syscall.UTF16PtrFromString(filepath.Join(root, "lockfile"))
	handle, err := syscall.CreateFile(path, syscall.GENERIC_WRITE, syscall.FILE_SHARE_READ, nil, syscall.CREATE_ALWAYS, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		t.Fatal(err)
	}
	if err := ensureProfileStopped(root); err == nil {
		syscall.CloseHandle(handle)
		t.Fatal("running browser accepted")
	}
	syscall.CloseHandle(handle)
	if err := ensureProfileStopped(root); err != nil {
		t.Fatalf("stale lock rejected: %v", err)
	}
}

func TestInjectionValidation(t *testing.T) {
	root := t.TempDir()
	extension := filepath.Join(root, "extension")
	if err := os.Mkdir(extension, 0700); err != nil {
		t.Fatal(err)
	}
	servers := json.RawMessage(`[{"urls":"stun:example.com:3478"}]`)
	for _, template := range []string{"no placeholder", "__ICE_SERVERS__ __ICE_SERVERS__"} {
		os.WriteFile(filepath.Join(extension, "inject.template.js"), []byte(template), 0600)
		if err := generateInjection(root, servers); err == nil {
			t.Fatal("bad template accepted")
		}
	}
	os.WriteFile(filepath.Join(extension, "inject.template.js"), []byte("const ice = __ICE_SERVERS__;"), 0600)
	if err := generateInjection(root, servers); err != nil {
		t.Fatal(err)
	}
	before, _ := os.ReadFile(filepath.Join(extension, "inject.js"))
	if err := generateInjection(root, json.RawMessage(`[null]`)); err == nil {
		t.Fatal("bad ICE accepted")
	}
	after, _ := os.ReadFile(filepath.Join(extension, "inject.js"))
	if !bytes.Equal(before, after) {
		t.Fatal("invalid ICE overwrote injection")
	}
}
