//go:build windows

package main

import (
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"unsafe"
)

var procMoveFileEx = kernel32.NewProc("MoveFileExW")

// os.Rename does not promise atomic replacement on Windows. MoveFileEx replaces
// the destination without deleting the previous valid file first.
func atomicWriteFile(path string, data []byte) error {
	f, err := os.CreateTemp(filepath.Dir(path), ".parsecwebturn-*.tmp")
	if err != nil {
		return err
	}
	temporary := f.Name()
	defer os.Remove(temporary)
	defer f.Close()
	if _, err := f.Write(data); err != nil {
		return err
	}
	if err := f.Sync(); err != nil {
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	from, err := syscall.UTF16PtrFromString(temporary)
	if err != nil {
		return err
	}
	to, err := syscall.UTF16PtrFromString(path)
	if err != nil {
		return err
	}
	const replaceExistingAndWriteThrough = 0x1 | 0x8
	ok, _, callErr := procMoveFileEx.Call(uintptr(unsafe.Pointer(from)), uintptr(unsafe.Pointer(to)), replaceExistingAndWriteThrough)
	if ok == 0 {
		return fmt.Errorf("cannot replace %s: %w", filepath.Base(path), callErr)
	}
	return nil
}
