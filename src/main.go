//go:build windows

package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unsafe"

	"github.com/lxn/walk"
	. "github.com/lxn/walk/declarative"
)

const (
	defaultTTL = 86400
	maxTTL     = 172800
)

type Settings struct {
	TurnKeyID      string `json:"turnKeyId"`
	EncryptedToken string `json:"encryptedApiToken"`
	TTL            int    `json:"ttl"`
}

type IceConfig struct {
	IceServers json.RawMessage `json:"iceServers"`
}

type dataBlob struct {
	cbData uint32
	pbData *byte
}

var (
	crypt32            = syscall.NewLazyDLL("crypt32.dll")
	kernel32           = syscall.NewLazyDLL("kernel32.dll")
	procCryptProtect   = crypt32.NewProc("CryptProtectData")
	procCryptUnprotect = crypt32.NewProc("CryptUnprotectData")
	procLocalFree      = kernel32.NewProc("LocalFree")
)

func appDir() string {
	exe, err := os.Executable()
	if err != nil {
		showFatal("Cannot locate executable: " + err.Error())
	}
	return filepath.Dir(exe)
}

func bytesToBlob(data []byte) dataBlob {
	if len(data) == 0 {
		return dataBlob{}
	}
	return dataBlob{cbData: uint32(len(data)), pbData: &data[0]}
}

func blobBytes(blob dataBlob) []byte {
	if blob.cbData == 0 || blob.pbData == nil {
		return nil
	}
	return unsafe.Slice(blob.pbData, blob.cbData)
}

func protectString(value string) (string, error) {
	in := bytesToBlob([]byte(value))
	var out dataBlob
	r, _, err := procCryptProtect.Call(
		uintptr(unsafe.Pointer(&in)),
		0,
		0,
		0,
		0,
		1, // CRYPTPROTECT_UI_FORBIDDEN
		uintptr(unsafe.Pointer(&out)),
	)
	if r == 0 {
		return "", err
	}
	defer procLocalFree.Call(uintptr(unsafe.Pointer(out.pbData)))
	return base64.StdEncoding.EncodeToString(blobBytes(out)), nil
}

func unprotectString(encoded string) (string, error) {
	raw, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return "", err
	}
	in := bytesToBlob(raw)
	var out dataBlob
	r, _, callErr := procCryptUnprotect.Call(
		uintptr(unsafe.Pointer(&in)),
		0,
		0,
		0,
		0,
		1, // CRYPTPROTECT_UI_FORBIDDEN
		uintptr(unsafe.Pointer(&out)),
	)
	if r == 0 {
		return "", callErr
	}
	defer procLocalFree.Call(uintptr(unsafe.Pointer(out.pbData)))
	return string(blobBytes(out)), nil
}

func settingsPath(root string) string {
	return filepath.Join(root, "settings.json")
}

func loadSettings(root string) (Settings, string, error) {
	var s Settings
	raw, err := os.ReadFile(settingsPath(root))
	if err != nil {
		return s, "", err
	}
	if err := json.Unmarshal(raw, &s); err != nil {
		return s, "", err
	}
	if s.TTL <= 0 {
		s.TTL = defaultTTL
	}
	token, err := unprotectString(s.EncryptedToken)
	if err != nil {
		return s, "", fmt.Errorf("cannot decrypt API token for this Windows user: %w", err)
	}
	return s, token, nil
}

func saveSettings(root string, keyID, token string, ttl int) error {
	keyID = strings.TrimSpace(keyID)
	token = strings.TrimSpace(token)
	if keyID == "" {
		return errors.New("TURN Key ID is required")
	}
	if token == "" {
		return errors.New("Cloudflare API token is required")
	}
	if ttl < 60 || ttl > maxTTL {
		return fmt.Errorf("TTL must be between 60 and %d seconds", maxTTL)
	}

	enc, err := protectString(token)
	if err != nil {
		return fmt.Errorf("DPAPI encryption failed: %w", err)
	}
	s := Settings{
		TurnKeyID:      keyID,
		EncryptedToken: enc,
		TTL:            ttl,
	}
	raw, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(settingsPath(root), raw, 0600)
}

func showSettings(root string, current Settings, currentToken string) bool {
	var mw *walk.MainWindow
	var keyEdit, tokenEdit *walk.LineEdit
	var ttlEdit *walk.NumberEdit
	saved := false

	ttl := current.TTL
	if ttl <= 0 {
		ttl = defaultTTL
	}

	err := (MainWindow{
		AssignTo: &mw,
		Title:    "ParsecWebTurn - Cloudflare TURN Settings",
		MinSize:  Size{Width: 520, Height: 300},
		Size:     Size{Width: 560, Height: 330},
		Layout:   VBox{MarginsZero: false},
		Children: []Widget{
			Label{Text: "Cloudflare Realtime TURN credentials"},
			Label{Text: "TURN Key ID:"},
			LineEdit{
				AssignTo: &keyEdit,
				Text:     current.TurnKeyID,
			},
			Label{Text: "TURN Key API Token (stored encrypted with Windows DPAPI):"},
			LineEdit{
				AssignTo:     &tokenEdit,
				Text:         currentToken,
				PasswordMode: true,
			},
			Label{Text: "Generated credential lifetime (seconds, max 172800 / 48h):"},
			NumberEdit{
				AssignTo: &ttlEdit,
				MinValue: 60,
				MaxValue: maxTTL,
				Value:    float64(ttl),
				Decimals: 0,
			},
			Label{
				Text: "The API token stays on this PC. ParsecWebTurn uses it only to request short-lived TURN credentials directly from Cloudflare.",
			},
			Composite{
				Layout: HBox{},
				Children: []Widget{
					HSpacer{},
					PushButton{
						Text: "Save && Start",
						OnClicked: func() {
							if err := saveSettings(root, keyEdit.Text(), tokenEdit.Text(), int(ttlEdit.Value())); err != nil {
								walk.MsgBox(mw, "ParsecWebTurn", err.Error(), walk.MsgBoxIconError)
								return
							}
							saved = true
							mw.Close()
						},
					},
					PushButton{
						Text: "Cancel",
						OnClicked: func() {
							mw.Close()
						},
					},
				},
			},
		},
	}).Create()
	if err != nil {
		walk.MsgBox(nil, "ParsecWebTurn", "Cannot create settings window:\\n"+err.Error(), walk.MsgBoxIconError)
		return false
	}

	mw.Run()
	return saved
}

func requestIceServers(keyID, token string, ttl int) (json.RawMessage, error) {
	endpoint := fmt.Sprintf(
		"https://rtc.live.cloudflare.com/v1/turn/keys/%s/credentials/generate-ice-servers",
		keyID,
	)

	body, _ := json.Marshal(map[string]int{"ttl": ttl})
	req, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "ParsecWebTurn/0.2.0")

	client := &http.Client{Timeout: 20 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("Cloudflare request failed: %w", err)
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("Cloudflare returned HTTP %d: %s", resp.StatusCode, strings.TrimSpace(string(raw)))
	}

	var cfg IceConfig
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return nil, fmt.Errorf("invalid Cloudflare response: %w", err)
	}
	var servers []any
	if err := json.Unmarshal(cfg.IceServers, &servers); err != nil || len(servers) == 0 {
		return nil, errors.New("Cloudflare response did not contain a usable iceServers array")
	}
	return cfg.IceServers, nil
}

func loadIceFallback(root string) (json.RawMessage, error) {
	raw, err := os.ReadFile(filepath.Join(root, "ice.json"))
	if err != nil {
		return nil, err
	}
	var cfg IceConfig
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return nil, err
	}
	var servers []any
	if err := json.Unmarshal(cfg.IceServers, &servers); err != nil || len(servers) == 0 {
		return nil, errors.New("ice.json does not contain a valid iceServers array")
	}
	return cfg.IceServers, nil
}

func generateInjection(root string, servers json.RawMessage) error {
	templatePath := filepath.Join(root, "extension", "inject.template.js")
	injectPath := filepath.Join(root, "extension", "inject.js")

	template, err := os.ReadFile(templatePath)
	if err != nil {
		return fmt.Errorf("cannot read extension\\\\inject.template.js: %w", err)
	}
	inject := strings.Replace(string(template), "__ICE_SERVERS__", string(servers), 1)
	if err := os.WriteFile(injectPath, []byte(inject), 0600); err != nil {
		return fmt.Errorf("cannot generate extension\\\\inject.js: %w", err)
	}
	return nil
}

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
		return errors.New("extension\\\\manifest.json is missing")
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
	return cmd.Start()
}

func showFatal(message string) {
	walk.MsgBox(nil, "ParsecWebTurn", message, walk.MsgBoxIconError)
	os.Exit(1)
}

func main() {
	root := appDir()
	forceSettings := false
	for _, arg := range os.Args[1:] {
		if strings.EqualFold(arg, "--settings") || strings.EqualFold(arg, "/settings") {
			forceSettings = true
		}
	}

	settings, token, err := loadSettings(root)
	if forceSettings || err != nil {
		if err != nil && !os.IsNotExist(err) {
			walk.MsgBox(nil, "ParsecWebTurn", "Stored settings could not be loaded:\\n"+err.Error(), walk.MsgBoxIconWarning)
		}
		if !showSettings(root, settings, token) {
			return
		}
		settings, token, err = loadSettings(root)
		if err != nil {
			showFatal("Cannot reload settings:\\n" + err.Error())
		}
	}

	servers, err := requestIceServers(settings.TurnKeyID, token, settings.TTL)
	if err != nil {
		choice := walk.MsgBox(
			nil,
			"ParsecWebTurn - Cloudflare TURN error",
			err.Error()+"\\n\\nRetry after editing Cloudflare settings?\\n\\nYes = open Settings\\nNo = try local ice.json fallback",
			walk.MsgBoxYesNo|walk.MsgBoxIconWarning,
		)
		if choice == walk.DlgCmdYes {
			if !showSettings(root, settings, token) {
				return
			}
			settings, token, err = loadSettings(root)
			if err != nil {
				showFatal("Cannot reload settings:
" + err.Error())
			}
			servers, err = requestIceServers(settings.TurnKeyID, token, settings.TTL)
		}
		if err != nil {
			servers, err = loadIceFallback(root)
			if err != nil {
				showFatal("Cloudflare credential generation failed and no usable ice.json fallback was found.\\n\\n" + err.Error())
			}
		}
	}

	if err := generateInjection(root, servers); err != nil {
		showFatal(err.Error())
	}
	if err := startEdge(root); err != nil {
		showFatal("Failed to start Edge:\\n" + err.Error())
	}
}
