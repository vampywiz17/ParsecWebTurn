//go:build windows

package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"unsafe"
)

type IceConfig struct {
	IceServers json.RawMessage `json:"iceServers"`
}

var (
	user32          = syscall.NewLazyDLL("user32.dll")
	procMessageBoxW = user32.NewProc("MessageBoxW")
)

func utf16Ptr(s string) *uint16 {
	p, _ := syscall.UTF16PtrFromString(s)
	return p
}

func msgBox(title, message string, flags uintptr) {
	procMessageBoxW.Call(
		0,
		uintptr(unsafe.Pointer(utf16Ptr(message))),
		uintptr(unsafe.Pointer(utf16Ptr(title))),
		flags,
	)
}

func fail(msg string) {
	msgBox("ParsecWebTurn", msg, 0x10)
	os.Exit(1)
}

func appDir() string {
	exe, err := os.Executable()
	if err != nil {
		fail("Cannot locate executable: " + err.Error())
	}
	return filepath.Dir(exe)
}

func findEdge() string {
	candidates := []string{}
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

func main() {
	root := appDir()
	configPath := filepath.Join(root, "ice.json")
	templatePath := filepath.Join(root, "extension", "inject.template.js")
	injectPath := filepath.Join(root, "extension", "inject.js")
	manifestPath := filepath.Join(root, "extension", "manifest.json")

	raw, err := os.ReadFile(configPath)
	if err != nil {
		fail("Cannot read ice.json:\n" + err.Error())
	}

	var cfg IceConfig
	if err := json.Unmarshal(raw, &cfg); err != nil {
		fail("ice.json is not valid JSON:\n" + err.Error())
	}
	if len(cfg.IceServers) == 0 || string(cfg.IceServers) == "null" {
		fail("ice.json does not contain an iceServers array.")
	}
	if strings.Contains(string(raw), "PASTE_CLOUDFLARE_TURN_") {
		fail("Edit ice.json first and paste your Cloudflare TURN username and credential.")
	}

	var servers []any
	if err := json.Unmarshal(cfg.IceServers, &servers); err != nil || len(servers) == 0 {
		fail("iceServers must be a non-empty JSON array.")
	}

	template, err := os.ReadFile(templatePath)
	if err != nil {
		fail("Cannot read extension\\inject.template.js:\n" + err.Error())
	}

	inject := strings.Replace(string(template), "__ICE_SERVERS__", string(cfg.IceServers), 1)
	if err := os.WriteFile(injectPath, []byte(inject), 0600); err != nil {
		fail("Cannot generate extension\\inject.js:\n" + err.Error())
	}

	if _, err := os.Stat(manifestPath); err != nil {
		fail("extension\\manifest.json is missing.")
	}

	edge := findEdge()
	if edge == "" {
		fail("Microsoft Edge was not found.\n\nThis portable launcher uses the Edge already installed on Windows.")
	}

	profile := filepath.Join(root, "Profile")
	if err := os.MkdirAll(profile, 0700); err != nil {
		fail("Cannot create Profile directory:\n" + err.Error())
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
		CreationFlags: 0x00000008,
	}

	if err := cmd.Start(); err != nil {
		fail(fmt.Sprintf("Failed to start Edge:\n%v", err))
	}
}
