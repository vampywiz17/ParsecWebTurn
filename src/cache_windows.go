//go:build windows

package main

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type credentialCache struct {
	Fingerprint string          `json:"fingerprint"`
	IssuedAt    time.Time       `json:"issuedAt"`
	ExpiresAt   time.Time       `json:"expiresAt"`
	Servers     json.RawMessage `json:"iceServers"`
}

func cacheFingerprint(s Settings, token string) string {
	raw, _ := json.Marshal([]any{s.TurnKeyID, token, s.TTL})
	return fmt.Sprintf("%x", sha256.Sum256(raw))
}

func cachePath(root string) string { return filepath.Join(root, ".turn-cache.json") }

func cacheUsable(cache credentialCache, s Settings, token string, now time.Time) bool {
	// Keep at least a quarter of the requested lifetime (minimum five minutes)
	// for the next session. Moving the local clock backwards invalidates reuse.
	margin := time.Duration(s.TTL/4) * time.Second
	if margin < 5*time.Minute {
		margin = 5 * time.Minute
	}
	return cache.Fingerprint == cacheFingerprint(s, token) &&
		!now.Before(cache.IssuedAt) && cache.ExpiresAt.Sub(now) > margin &&
		cache.ExpiresAt.Sub(cache.IssuedAt) == time.Duration(s.TTL)*time.Second
}

func loadCredentialCache(root string, s Settings, token string, now time.Time) (json.RawMessage, error) {
	raw, err := os.ReadFile(cachePath(root))
	if err != nil {
		return nil, err
	}
	var envelope struct {
		Encrypted string `json:"encryptedCredentials"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil {
		return nil, err
	}
	plain, err := unprotectString(envelope.Encrypted)
	if err != nil {
		return nil, err
	}
	var cache credentialCache
	if err := json.Unmarshal([]byte(plain), &cache); err != nil {
		return nil, err
	}
	if !cacheUsable(cache, s, token, now) {
		return nil, errors.New("credentials need refreshing")
	}
	return parseAndValidateICE(append(append([]byte(`{"iceServers":`), cache.Servers...), '}'))
}

func saveCredentialCache(root string, s Settings, token string, servers json.RawMessage, issued time.Time) error {
	cache := credentialCache{cacheFingerprint(s, token), issued, issued.Add(time.Duration(s.TTL) * time.Second), servers}
	raw, err := json.Marshal(cache)
	if err != nil {
		return err
	}
	encrypted, err := protectString(string(raw))
	if err != nil {
		return err
	}
	envelope, err := json.Marshal(struct {
		Encrypted string `json:"encryptedCredentials"`
	}{encrypted})
	if err != nil {
		return err
	}
	return atomicWriteFile(cachePath(root), envelope)
}

func getIceServers(ctx context.Context, root string, s Settings, token string) (json.RawMessage, error) {
	if err := validateSettings(s.TurnKeyID, token, s.TTL); err != nil {
		return nil, err
	}
	if s.CacheCredentials {
		if servers, err := loadCredentialCache(root, s, token, time.Now()); err == nil {
			return servers, nil
		}
	}
	// Use the time before the request so network latency never extends expiry.
	issued := time.Now()
	servers, err := requestIceServers(ctx, cloudflareClient, cloudflareEndpoint(s.TurnKeyID), token, s.TTL)
	if err != nil {
		return nil, err
	}
	if s.CacheCredentials {
		// Caching is optional; inability to persist it must not block a valid session.
		_ = saveCredentialCache(root, s, token, servers, issued)
	}
	return servers, nil
}
