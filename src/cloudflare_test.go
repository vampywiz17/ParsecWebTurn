package main

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestCredentialRequest(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.Header.Get("Authorization") != "Bearer test-token" || r.Header.Get("User-Agent") != "ParsecWebTurn/"+version {
			t.Errorf("unexpected request method/headers")
		}
		body, _ := io.ReadAll(r.Body)
		if string(body) != `{"ttl":86400}` {
			t.Errorf("unexpected body: %s", body)
		}
		w.WriteHeader(http.StatusCreated)
		io.WriteString(w, `{"iceServers":[{"urls":"turn:example.com:3478","username":"user","credential":"secret"}]}`)
	}))
	defer server.Close()
	result, err := requestIceServers(context.Background(), server.Client(), server.URL, "test-token", 86400)
	if err != nil || !strings.Contains(string(result), "turn:example.com") {
		t.Fatalf("result=%s err=%v", result, err)
	}
}

func TestCredentialRequestFailures(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
	}{
		{"authentication", 401, "reflected-sensitive-token"},
		{"malformed ICE", 200, `{"iceServers":[null]}`},
		{"oversize", 200, strings.Repeat(" ", (1<<20)+1)},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(tc.status); io.WriteString(w, tc.body) }))
			defer server.Close()
			_, err := requestIceServers(context.Background(), server.Client(), server.URL, "test-token", 86400)
			if err == nil {
				t.Fatal("expected failure")
			}
			if strings.Contains(err.Error(), "reflected-sensitive-token") {
				t.Fatal("response body leaked into error")
			}
		})
	}
}

func TestCredentialRequestCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := requestIceServers(ctx, cloudflareClient, "https://example.invalid", "test-token", 60)
	if err == nil {
		t.Fatal("expected cancelled request")
	}
}

func TestCredentialRequestDoesNotFollowRedirect(t *testing.T) {
	visited := false
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { visited = true }))
	defer target.Close()
	source := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, target.URL, http.StatusTemporaryRedirect)
	}))
	defer source.Close()
	_, err := requestIceServers(context.Background(), cloudflareClient, source.URL, "test-token", 60)
	if err == nil || visited {
		t.Fatalf("redirect followed=%v, err=%v", visited, err)
	}
}
