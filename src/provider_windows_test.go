//go:build windows

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"testing"
)

func TestCustomProviderRoundTrip(t *testing.T) {
	root := t.TempDir()
	s := Settings{Provider: providerCustom, CustomURLs: []string{"stun:turn.example.com:3478", "turn:turn.example.com:3478?transport=udp", "turns:turn.example.com:5349?transport=tcp"}, CustomUsername: "test-user", TTL: defaultTTL}
	password := " password with spaces "
	if err := saveConfiguration(root, s, "", password); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(settingsPath(root))
	if bytes.Contains(raw, []byte(password)) {
		t.Fatal("custom password saved as plaintext")
	}
	loaded, secret, err := loadSettings(root)
	if err != nil || secret != password || loaded.Provider != providerCustom {
		t.Fatalf("custom settings did not round trip: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	servers, err := resolveIceServers(ctx, root, loaded, secret)
	if err != nil {
		t.Fatalf("custom provider must not contact Cloudflare: %v", err)
	}
	var decoded []iceServer
	if err := json.Unmarshal(servers, &decoded); err != nil {
		t.Fatal(err)
	}
	if len(decoded) != 1 || len(decoded[0].URLs) != 3 || decoded[0].Credential != password {
		t.Fatal("custom ICE configuration lost URLs or credentials")
	}
}

func TestCustomSTUNOnlyAndInvalidSettings(t *testing.T) {
	root := t.TempDir()
	s := Settings{Provider: providerCustom, CustomURLs: []string{"stun:stun.example.com:3478"}}
	if err := saveConfiguration(root, s, "", ""); err != nil {
		t.Fatal(err)
	}
	if _, secret, err := loadSettings(root); err != nil || secret != "" {
		t.Fatalf("STUN-only config rejected: %v", err)
	}
	for _, tc := range []Settings{
		{Provider: "unknown"},
		{Provider: providerCustom},
		{Provider: providerCustom, CustomURLs: []string{"https://example.com"}},
		{Provider: providerCustom, CustomURLs: []string{"turn:example.com:3478"}, CustomUsername: "user"},
	} {
		if err := saveConfiguration(root, tc, "", ""); err == nil {
			t.Fatalf("invalid custom configuration accepted: %+v", tc)
		}
	}
}

func TestProviderSwitchPreservesInactiveSecrets(t *testing.T) {
	root := t.TempDir()
	if err := saveSettings(root, "key-id", "cloud-token", defaultTTL, true); err != nil {
		t.Fatal(err)
	}
	s, _, err := loadSettings(root)
	if err != nil {
		t.Fatal(err)
	}
	s.Provider = providerCustom
	s.CustomURLs = []string{"turn:example.com:3478"}
	s.CustomUsername = "user"
	if err := saveConfiguration(root, s, "", "custom-password"); err != nil {
		t.Fatal(err)
	}
	s, _, err = loadSettings(root)
	if err != nil {
		t.Fatal(err)
	}
	cloudToken, err := unprotectString(s.EncryptedToken)
	if err != nil || cloudToken != "cloud-token" {
		t.Fatal("switching to custom lost the Cloudflare token")
	}
	s.Provider = providerCloudflare
	if err := saveConfiguration(root, s, cloudToken, ""); err != nil {
		t.Fatal(err)
	}
	s, _, err = loadSettings(root)
	if err != nil {
		t.Fatal(err)
	}
	password, err := unprotectString(s.EncryptedCustomPassword)
	if err != nil || password != "custom-password" {
		t.Fatal("switching to Cloudflare lost custom credentials")
	}
}

func TestLegacyCloudflareAndInactiveToken(t *testing.T) {
	root := t.TempDir()
	if err := saveSettings(root, "key-id", "cloud-token", defaultTTL, false); err != nil {
		t.Fatal(err)
	}
	s, _, _ := loadSettings(root)
	s.Provider = ""
	raw, _ := json.Marshal(s)
	atomicWriteFile(settingsPath(root), raw)
	loaded, secret, err := loadSettings(root)
	if err != nil || loaded.Provider != providerCloudflare || secret != "cloud-token" {
		t.Fatal("legacy settings migration failed")
	}
	s.Provider = providerCustom
	s.CustomURLs = []string{"stun:example.com:3478"}
	s.EncryptedToken = "corrupt inactive token"
	raw, _ = json.Marshal(s)
	atomicWriteFile(settingsPath(root), raw)
	if _, _, err := loadSettings(root); err != nil {
		t.Fatalf("inactive token blocked custom mode: %v", err)
	}
}
