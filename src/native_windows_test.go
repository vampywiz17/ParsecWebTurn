//go:build windows

package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestNativeSettingsWindow(t *testing.T) {
	if os.Getenv("PARSECWEBTURN_NATIVE_TESTS") != "1" {
		t.Skip("enable native Windows smoke tests with PARSECWEBTURN_NATIVE_TESTS=1")
	}
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	window, saved, err := createSettingsWindow(t.TempDir(), Settings{TTL: defaultTTL}, "")
	if err != nil {
		t.Fatal(err)
	}
	defer window.Dispose()
	if *saved {
		t.Fatal("settings were saved without user action")
	}
	if window.Title() != "ParsecWebTurn - Cloudflare TURN Settings" {
		t.Fatal("unexpected window title")
	}
}

func TestRealEdgeProfileLock(t *testing.T) {
	if os.Getenv("PARSECWEBTURN_NATIVE_TESTS") != "1" {
		t.Skip("enable native Windows smoke tests with PARSECWEBTURN_NATIVE_TESTS=1")
	}
	edge := findEdge()
	if edge == "" {
		t.Fatal("Microsoft Edge is required for native smoke tests")
	}
	profile := filepath.Join(t.TempDir(), "Profile")
	cmd := exec.Command(edge, "--headless=new", "--user-data-dir="+profile, "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0", "about:blank")
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	waited := make(chan error, 1)
	go func() { waited <- cmd.Wait() }()
	endpoint := ""
	defer func() {
		// Browser.close shuts down the test browser and its children gracefully.
		// Killing only the starter process can leave the actual browser running.
		if endpoint != "" {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			script := `let sent=false;const timer=setTimeout(()=>process.exit(1),8000);const ws=new WebSocket(process.argv[1]);ws.addEventListener('open',()=>{sent=true;ws.send(JSON.stringify({id:1,method:'Browser.close'}));});ws.addEventListener('close',()=>{clearTimeout(timer);if(!sent)process.exit(1);});ws.addEventListener('error',()=>{if(!sent)process.exit(1);});`
			if output, err := exec.CommandContext(ctx, "node", "-e", script, endpoint).CombinedOutput(); err != nil {
				t.Errorf("test browser shutdown failed: %v %s", err, output)
			}
		}
		select {
		case <-waited:
		case <-time.After(10 * time.Second):
			_ = cmd.Process.Kill()
			<-waited
			t.Error("test browser starter did not exit")
		}
		deadline := time.Now().Add(10 * time.Second)
		for time.Now().Before(deadline) {
			active, _ := profileActive(profile)
			if !active {
				time.Sleep(500 * time.Millisecond)
				return
			}
			time.Sleep(100 * time.Millisecond)
		}
		t.Error("test browser did not release its profile")
	}()
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		active, err := profileActive(profile)
		if err != nil {
			t.Fatal(err)
		}
		if active {
			data, err := os.ReadFile(filepath.Join(profile, "DevToolsActivePort"))
			parts := strings.Split(strings.TrimSpace(string(data)), "\n")
			if err != nil || len(parts) != 2 {
				time.Sleep(100 * time.Millisecond)
				continue
			}
			endpoint = "ws://127.0.0.1:" + strings.TrimSpace(parts[0]) + strings.TrimSpace(parts[1])
			if err := ensureProfileStopped(profile); err == nil {
				t.Fatal("real running Edge profile was accepted")
			}
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Fatal("Edge did not lock its disposable profile")
}
