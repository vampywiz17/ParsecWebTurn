//go:build windows

package main

import (
	"encoding/base64"
	"syscall"
	"unsafe"
)

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
