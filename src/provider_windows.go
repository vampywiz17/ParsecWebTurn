//go:build windows

package main

import (
	"context"
	"encoding/json"
	"strings"
)

// coturn and eturnal speak standard TURN. They need no vendor-specific client:
// the same browser ICE configuration supports static or externally generated credentials.
func customIceServers(urls []string, username, password string) (json.RawMessage, error) {
	cleaned := make([]string, 0, len(urls))
	for _, url := range urls {
		if url = strings.TrimSpace(url); url != "" {
			cleaned = append(cleaned, url)
		}
	}
	raw, err := json.Marshal(struct {
		Servers []iceServer `json:"iceServers"`
	}{
		[]iceServer{{URLs: cleaned, Username: username, Credential: password}},
	})
	if err != nil {
		return nil, err
	}
	return parseAndValidateICE(raw)
}

func resolveIceServers(ctx context.Context, root string, s Settings, secret string) (json.RawMessage, error) {
	if err := validateConfiguration(s, secret); err != nil {
		return nil, err
	}
	if s.Provider == providerCustom {
		return customIceServers(s.CustomURLs, s.CustomUsername, secret)
	}
	return getIceServers(ctx, root, s, secret)
}
