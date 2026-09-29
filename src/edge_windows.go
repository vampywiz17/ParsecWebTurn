//go:build windows

package main

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"time"
)

func findEdge() string {
	var candidates []string
	if p := os.Getenv("ProgramFiles(x86)"); p != "" {
		candidates = append(candidates, filepath.Join(p, "Microsoft", "Edge", "Application", "msedge.exe"))
	}
	if p := os.Getenv("ProgramFiles"); p != "" {
		candidates = append(candidates, filepath.Join(p, "Microsoft", "Edge", "Application", "msedge.exe"))
	}
	if p := os.Getenv("LOCALAPPDATA"); p != "" {
		candidates = append(candidates, filepath.Join(p, "Microsoft", "Edge", "Application", "msedge.exe"))
	}
	for _, p := range candidates {
		if st, err := os.Stat(p); err == nil && !st.IsDir() {
			return p
		}
	}
	if p, err := exec.LookPath("msedge.exe"); err == nil {
		return p
	}
	return ""
}

func startEdge(root string) error {
	if _, err := os.Stat(filepath.Join(root, "extension", "manifest.json")); err != nil {
		return errors.New("extension/manifest.json is missing")
	}
	edge := findEdge()
	if edge == "" {
		return errors.New("Microsoft Edge was not found")
	}

	profile := filepath.Join(root, "Profile")
	if err := os.MkdirAll(profile, 0700); err != nil {
		return fmt.Errorf("cannot create Profile directory: %w", err)
	}

	extension := filepath.Join(root, "extension")
	args := []string{
		"--user-data-dir=" + profile,
		"--no-first-run",
		"--no-default-browser-check",
		"--disable-extensions-except=" + extension,
		"--load-extension=" + extension,
		"--app=https://web.parsec.app/",
	}

	cmd := exec.Command(edge, args...)
	cmd.Dir = root
	cmd.SysProcAttr = &syscall.SysProcAttr{
		HideWindow:    true,
		CreationFlags: 0x00000008, // DETACHED_PROCESS
	}
	if err := ensureProfileStopped(profile); err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	defer cmd.Process.Release()
	// Keep the launcher lock until Edge owns the profile. Otherwise two rapid
	// launches could rewrite inject.js while the first browser is starting.
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		active, err := profileActive(profile)
		if err != nil {
			return err
		}
		if active {
			return nil
		}
		time.Sleep(100 * time.Millisecond)
	}
	return errors.New("Edge was started, but its profile could not be confirmed within 15 seconds. Close any ParsecWebTurn Edge windows before retrying")
}
