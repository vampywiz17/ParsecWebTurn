package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"regexp"
	"strconv"
	"strings"
)

type iceServer struct {
	URLs       []string `json:"urls"`
	Username   string   `json:"username,omitempty"`
	Credential string   `json:"credential,omitempty"`
}

var iceURLPattern = regexp.MustCompile(`^(stun|stuns|turn|turns):(\[[0-9a-fA-F:.]+\]|[a-zA-Z0-9.-]+)(?::([0-9]+))?(?:\?transport=(udp|tcp))?$`)

// parseAndValidateICE normalizes both supported URL forms and rejects unusable
// entries before they reach the browser. Port 53 is blocked by web browsers.
func parseAndValidateICE(raw []byte) (json.RawMessage, error) {
	var cfg struct {
		Servers []json.RawMessage `json:"iceServers"`
	}
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return nil, fmt.Errorf("invalid ICE JSON: %w", err)
	}
	if len(cfg.Servers) == 0 {
		return nil, errors.New("iceServers must be a non-empty array")
	}
	servers := make([]iceServer, 0, len(cfg.Servers))
	for i, entry := range cfg.Servers {
		var input *struct {
			URLs           json.RawMessage `json:"urls"`
			Username       string          `json:"username"`
			Credential     string          `json:"credential"`
			CredentialType string          `json:"credentialType"`
		}
		if err := json.Unmarshal(entry, &input); err != nil || input == nil {
			return nil, fmt.Errorf("iceServers[%d] must be a server object", i)
		}
		if input.CredentialType != "" && input.CredentialType != "password" {
			return nil, fmt.Errorf("iceServers[%d]: only password credentials are supported", i)
		}
		var urls []string
		if err := json.Unmarshal(input.URLs, &urls); err != nil {
			var single string
			if err := json.Unmarshal(input.URLs, &single); err != nil {
				return nil, fmt.Errorf("iceServers[%d].urls must be a string or string array", i)
			}
			urls = []string{single}
		}
		if len(urls) == 0 {
			return nil, fmt.Errorf("iceServers[%d].urls cannot be empty", i)
		}
		server := iceServer{Username: input.Username, Credential: input.Credential}
		for _, url := range urls {
			parts := iceURLPattern.FindStringSubmatch(url)
			if parts == nil {
				return nil, fmt.Errorf("iceServers[%d] contains an invalid STUN/TURN URL", i)
			}
			host := parts[2]
			if strings.HasPrefix(host, "[") {
				if net.ParseIP(strings.Trim(host, "[]")) == nil {
					return nil, fmt.Errorf("iceServers[%d] contains an invalid IPv6 address", i)
				}
			} else if strings.HasPrefix(host, ".") || strings.Contains(host, "..") || host == "-" {
				return nil, fmt.Errorf("iceServers[%d] contains an invalid hostname", i)
			}
			if parts[3] != "" {
				port, err := strconv.Atoi(parts[3])
				if err != nil || port < 1 || port > 65535 {
					return nil, fmt.Errorf("iceServers[%d] contains an invalid port", i)
				}
				if port == 53 {
					continue
				}
			}
			if strings.HasPrefix(parts[1], "turn") && (strings.TrimSpace(server.Username) == "" || strings.TrimSpace(server.Credential) == "") {
				return nil, fmt.Errorf("iceServers[%d]: TURN requires a username and credential", i)
			}
			server.URLs = append(server.URLs, url)
		}
		if len(server.URLs) > 0 {
			servers = append(servers, server)
		}
	}
	if len(servers) == 0 {
		return nil, errors.New("no browser-compatible ICE URLs remain")
	}
	return json.Marshal(servers)
}
