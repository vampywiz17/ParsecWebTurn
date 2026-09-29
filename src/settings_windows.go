//go:build windows

package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	defaultTTL = 86400
	maxTTL     = 172800
)

type Settings struct {
	TurnKeyID        string `json:"turnKeyId"`
	EncryptedToken   string `json:"encryptedApiToken"`
	CacheCredentials bool   `json:"cacheCredentials"`
	TTL              int    `json:"ttl"`
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
	if s.TTL == 0 {
		s.TTL = defaultTTL
	}
	token, err := unprotectString(s.EncryptedToken)
	if err != nil {
		return s, "", fmt.Errorf("cannot decrypt API token for this Windows user: %w", err)
	}
	if err := validateSettings(s.TurnKeyID, token, s.TTL); err != nil {
		return s, token, err
	}
	return s, token, nil
}

func saveSettings(root string, keyID, token string, ttl int, cacheCredentials bool) error {
	keyID = strings.TrimSpace(keyID)
	token = strings.TrimSpace(token)
	if err := validateSettings(keyID, token, ttl); err != nil {
		return err
	}

	enc, err := protectString(token)
	if err != nil {
		return fmt.Errorf("DPAPI encryption failed: %w", err)
	}
	s := Settings{
		TurnKeyID:        keyID,
		CacheCredentials: cacheCredentials,
		EncryptedToken:   enc,
		TTL:              ttl,
	}
	raw, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return atomicWriteFile(settingsPath(root), raw)
}

func validateSettings(keyID, token string, ttl int) error {
	if keyID == "" || strings.TrimSpace(keyID) != keyID {
		return errors.New("TURN Key ID is required and cannot have surrounding spaces")
	}
	for _, c := range keyID {
		if !((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_') {
			return errors.New("TURN Key ID may contain only letters, digits, hyphens and underscores")
		}
	}
	if strings.TrimSpace(token) == "" || strings.ContainsAny(token, "\r\n") {
		return errors.New("A valid Cloudflare API token is required")
	}
	if ttl < 60 || ttl > maxTTL {
		return fmt.Errorf("TTL must be between 60 and %d seconds", maxTTL)
	}
	return nil
}
