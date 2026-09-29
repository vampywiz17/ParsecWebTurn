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
	defaultTTL         = 86400
	maxTTL             = 172800
	providerCloudflare = "cloudflare"
	providerCustom     = "custom"
)

type Settings struct {
	Provider                string   `json:"provider,omitempty"`
	CustomURLs              []string `json:"customUrls,omitempty"`
	CustomUsername          string   `json:"customUsername,omitempty"`
	EncryptedCustomPassword string   `json:"encryptedCustomPassword,omitempty"`
	TurnKeyID               string   `json:"turnKeyId"`
	EncryptedToken          string   `json:"encryptedApiToken"`
	CacheCredentials        bool     `json:"cacheCredentials"`
	TTL                     int      `json:"ttl"`
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
	if s.Provider == "" {
		s.Provider = providerCloudflare
	}
	if s.Provider != providerCloudflare && s.Provider != providerCustom {
		return s, "", errors.New("Unknown TURN provider; choose Cloudflare or Custom")
	}
	encrypted := s.EncryptedToken
	if s.Provider == providerCustom {
		encrypted = s.EncryptedCustomPassword
	}
	var token string
	if encrypted != "" {
		token, err = unprotectString(encrypted)
	}
	if err != nil {
		return s, "", fmt.Errorf("cannot decrypt credentials for this Windows user: %w", err)
	}
	if err := validateConfiguration(s, token); err != nil {
		return s, token, err
	}
	return s, token, nil
}

func saveSettings(root string, keyID, token string, ttl int, cacheCredentials bool) error {
	return saveConfiguration(root, Settings{Provider: providerCloudflare, TurnKeyID: keyID, TTL: ttl, CacheCredentials: cacheCredentials}, token, "")
}

func saveConfiguration(root string, s Settings, cloudToken, customPassword string) error {
	if s.Provider == "" {
		s.Provider = providerCloudflare
	}
	s.TurnKeyID = strings.TrimSpace(s.TurnKeyID)
	cloudToken = strings.TrimSpace(cloudToken)
	s.CustomUsername = strings.TrimSpace(s.CustomUsername)
	secret := cloudToken
	if s.Provider == providerCustom {
		secret = customPassword
	}
	if err := validateConfiguration(s, secret); err != nil {
		return err
	}
	if cloudToken != "" {
		enc, err := protectString(cloudToken)
		if err != nil {
			return fmt.Errorf("DPAPI encryption failed: %w", err)
		}
		s.EncryptedToken = enc
	}
	if customPassword != "" {
		enc, err := protectString(customPassword)
		if err != nil {
			return fmt.Errorf("DPAPI encryption failed: %w", err)
		}
		s.EncryptedCustomPassword = enc
	} else if s.Provider == providerCustom {
		s.EncryptedCustomPassword = ""
	}
	if s.Provider == providerCustom {
		servers, _ := customIceServers(s.CustomURLs, s.CustomUsername, customPassword)
		var normalized []iceServer
		if err := json.Unmarshal(servers, &normalized); err != nil {
			return err
		}
		s.CustomURLs = normalized[0].URLs
	}
	raw, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	return atomicWriteFile(settingsPath(root), raw)
}

func validateConfiguration(s Settings, secret string) error {
	switch s.Provider {
	case "", providerCloudflare:
		return validateSettings(s.TurnKeyID, secret, s.TTL)
	case providerCustom:
		_, err := customIceServers(s.CustomURLs, s.CustomUsername, secret)
		return err
	default:
		return errors.New("Unknown TURN provider; choose Cloudflare or Custom")
	}
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
