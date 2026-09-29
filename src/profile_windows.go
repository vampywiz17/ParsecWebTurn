//go:build windows

package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

const errorSharingViolation = syscall.Errno(32)

func acquireLauncherLock(root string) (func(), error) {
	path, err := syscall.UTF16PtrFromString(filepath.Join(root, ".launcher.lock"))
	if err != nil {
		return nil, err
	}
	handle, err := syscall.CreateFile(path, syscall.GENERIC_WRITE, 0, nil, syscall.OPEN_ALWAYS, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		if errors.Is(err, errorSharingViolation) {
			return nil, errors.New("ParsecWebTurn is already starting. Please wait for the other launcher to finish")
		}
		return nil, fmt.Errorf("cannot lock launcher: %w", err)
	}
	return func() { syscall.CloseHandle(handle) }, nil
}

// Chromium's Windows ProcessSingleton holds Profile/lockfile for writing with
// FILE_SHARE_READ until the browser exits. Probe it without creating/truncating
// anything; a stale, unlocked file does not prevent startup.
func profileActive(profile string) (bool, error) {
	path, err := syscall.UTF16PtrFromString(filepath.Join(profile, "lockfile"))
	if err != nil {
		return false, err
	}
	handle, err := syscall.CreateFile(path, syscall.GENERIC_WRITE, syscall.FILE_SHARE_READ|syscall.FILE_SHARE_WRITE|syscall.FILE_SHARE_DELETE, nil, syscall.OPEN_EXISTING, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		if errors.Is(err, errorSharingViolation) {
			return true, nil
		}
		if os.IsNotExist(err) {
			return false, nil
		}
		return false, fmt.Errorf("cannot check Edge profile: %w", err)
	}
	syscall.CloseHandle(handle)
	return false, nil
}

func ensureProfileStopped(profile string) error {
	active, err := profileActive(profile)
	if err != nil {
		return err
	}
	if active {
		return errors.New("The ParsecWebTurn Edge profile is already running.\n\nClose its Parsec windows, then start ParsecWebTurn again so the current TURN configuration is loaded.\n\nIf Edge keeps running in the background, disable Startup boost and background extensions in this dedicated profile")
	}
	return nil
}
