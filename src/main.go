//go:build windows

package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/lxn/walk"
)

func appDir() string {
	exe, err := os.Executable()
	if err != nil {
		showFatal("Cannot locate executable: " + err.Error())
	}
	return filepath.Dir(exe)
}

func showFatal(message string) {
	walk.MsgBox(nil, "ParsecWebTurn", message, walk.MsgBoxIconError)
	os.Exit(1)
}

func main() {
	runtime.LockOSThread()
	root := appDir()
	unlock, err := acquireLauncherLock(root)
	if err != nil {
		showFatal(err.Error())
	}
	defer unlock()
	if err := ensureProfileStopped(filepath.Join(root, "Profile")); err != nil {
		showFatal(err.Error())
	}
	forceSettings := false
	for _, arg := range os.Args[1:] {
		if strings.EqualFold(arg, "--settings") || strings.EqualFold(arg, "/settings") {
			forceSettings = true
		}
	}

	settings, token, err := loadSettings(root)
	if forceSettings || err != nil {
		if err != nil && !os.IsNotExist(err) {
			walk.MsgBox(nil, "ParsecWebTurn", "Stored settings could not be loaded:\n"+err.Error(), walk.MsgBoxIconWarning)
		}
		if !showSettings(root, settings, token) {
			return
		}
		settings, token, err = loadSettings(root)
		if err != nil {
			showFatal("Cannot reload settings:\n" + err.Error())
		}
	}

	servers, err := getIceServers(context.Background(), root, settings, token)
	if err != nil {
		choice := walk.MsgBox(
			nil,
			"ParsecWebTurn - Cloudflare TURN error",
			err.Error()+"\n\nRetry after editing Cloudflare settings?\n\nYes = open Settings\nNo = try local ice.json fallback",
			walk.MsgBoxYesNo|walk.MsgBoxIconWarning,
		)
		if choice == walk.DlgCmdYes {
			if !showSettings(root, settings, token) {
				return
			}
			settings, token, err = loadSettings(root)
			if err != nil {
				showFatal("Cannot reload settings:\n" + err.Error())
			}
			servers, err = getIceServers(context.Background(), root, settings, token)
		}
		if err != nil {
			cloudflareErr := err
			servers, err = loadIceFallback(root)
			if err != nil {
				showFatal(fmt.Sprintf("Cloudflare credential generation failed and no usable ice.json fallback was found.\n\nCloudflare: %v\nFallback: %v", cloudflareErr, err))
			}
		}
	}

	if err := ensureProfileStopped(filepath.Join(root, "Profile")); err != nil {
		showFatal(err.Error())
	}
	if err := generateInjection(root, servers); err != nil {
		showFatal(err.Error())
	}
	if err := startEdge(root); err != nil {
		showFatal("Failed to start Edge:\n" + err.Error())
	}
}
