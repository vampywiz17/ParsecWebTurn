//go:build windows

package main

import (
	"context"
	"image/png"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/lxn/walk"
)

func settingsWidget(container walk.Container, name string) walk.Widget {
	children := container.Children()
	for i := 0; i < children.Len(); i++ {
		child := children.At(i)
		if child.Name() == name {
			return child
		}
		if nested, ok := child.(walk.Container); ok {
			if found := settingsWidget(nested, name); found != nil {
				return found
			}
		}
	}
	return nil
}

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
	if window.Title() != "ParsecWebTurn - Connection Settings" {
		t.Fatal("unexpected window title")
	}
	provider := settingsWidget(window, "provider").(*walk.ComboBox)
	for _, index := range []int{1, 0} {
		if err := provider.SetCurrentIndex(index); err != nil {
			t.Fatal(err)
		}
	}
	if directory := os.Getenv("PARSECWEBTURN_PREVIEW_DIR"); directory != "" {
		// Render only this test window, away from the user's desktop.
		window.SetBounds(walk.Rectangle{X: -30000, Y: -30000, Width: 760, Height: 650})
		capture := func(index int) {
			bitmap, err := walk.NewBitmapFromWindow(window)
			if err != nil {
				t.Error(err)
				window.Close()
				return
			}
			image, err := bitmap.ToImage()
			bitmap.Dispose()
			if err != nil {
				t.Error(err)
				window.Close()
				return
			}
			name := "settings-cloudflare.png"
			// GDI window printing does not set alpha; the rendered window is opaque.
			for offset := 3; offset < len(image.Pix); offset += 4 {
				image.Pix[offset] = 255
			}
			if index == 1 {
				name = "settings-custom.png"
			}
			file, err := os.Create(filepath.Join(directory, name))
			if err != nil {
				t.Error(err)
				window.Close()
				return
			}
			err = png.Encode(file, image)
			file.Close()
			if err != nil {
				t.Error(err)
			}
		}
		time.AfterFunc(350*time.Millisecond, func() {
			window.Synchronize(func() {
				capture(0)
				provider.SetCurrentIndex(1)
				time.AfterFunc(350*time.Millisecond, func() { window.Synchronize(func() { capture(1); window.Close() }) })
			})
		})
		window.Run()
	}
}

func TestNativeCustomSettingsSave(t *testing.T) {
	if os.Getenv("PARSECWEBTURN_NATIVE_TESTS") != "1" {
		t.Skip("enable native Windows smoke tests")
	}
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	root := t.TempDir()
	window, saved, err := createSettingsWindow(root, Settings{TTL: defaultTTL}, "")
	if err != nil {
		t.Fatal(err)
	}
	defer window.Dispose()
	settingsWidget(window, "provider").(*walk.ComboBox).SetCurrentIndex(1)
	settingsWidget(window, "urls").(*walk.TextEdit).SetText("turn:example.com:3478?transport=udp\r\nturns:example.com:5349?transport=tcp")
	settingsWidget(window, "username").(*walk.LineEdit).SetText("native-test-user")
	settingsWidget(window, "password").(*walk.LineEdit).SetText("native-test-password")
	// BN_CLICKED exercises the real Walk button handler without starting a browser.
	window.SetBounds(walk.Rectangle{X: -30000, Y: -30000, Width: 760, Height: 650})
	time.AfterFunc(350*time.Millisecond, func() {
		window.Synchronize(func() {
			button := settingsWidget(window, "save")
			button.SendMessage(0x0111, 0, uintptr(button.Handle()))
			if !*saved {
				window.Close()
			}
		})
	})
	window.Run()
	if !*saved {
		t.Fatal("custom form did not save")
	}
	config, secret, err := loadSettings(root)
	if err != nil || config.Provider != providerCustom || len(config.CustomURLs) != 2 || secret != "native-test-password" {
		t.Fatalf("custom UI saved wrong settings: %v", err)
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
