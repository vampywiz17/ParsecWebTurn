package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// The shared build script supplies the release version with -ldflags.
var version = "dev"

func requestIceServers(ctx context.Context, client *http.Client, endpoint, token string, ttl int) (json.RawMessage, error) {
	body, _ := json.Marshal(map[string]int{"ttl": ttl})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "ParsecWebTurn/"+version)

	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("Cloudflare request failed: %w", err)
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(io.LimitReader(resp.Body, (1<<20)+1))
	if err != nil {
		return nil, err
	}
	if len(raw) > 1<<20 {
		return nil, fmt.Errorf("Cloudflare response exceeds 1 MiB")
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("Cloudflare returned HTTP %d; check credentials, connectivity or service availability", resp.StatusCode)
	}

	servers, err := parseAndValidateICE(raw)
	if err != nil {
		return nil, fmt.Errorf("invalid Cloudflare response: %w", err)
	}
	return servers, nil
}

var cloudflareClient = &http.Client{
	Timeout: 20 * time.Second,
	// Never forward the bearer token through an unexpected redirect.
	CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
}

func cloudflareEndpoint(keyID string) string {
	return fmt.Sprintf("https://rtc.live.cloudflare.com/v1/turn/keys/%s/credentials/generate-ice-servers", keyID)
}
