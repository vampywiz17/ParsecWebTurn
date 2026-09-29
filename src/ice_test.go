package main

import (
	"encoding/json"
	"testing"
)

func TestICEValidation(t *testing.T) {
	cases := []struct {
		name, raw string
		valid     bool
	}{
		{"string URL", `{"iceServers":[{"urls":"stun:stun.cloudflare.com:3478"}]}`, true},
		{"TURN array", `{"iceServers":[{"urls":["turn:turn.cloudflare.com:3478?transport=udp","turns:turn.cloudflare.com:443?transport=tcp"],"username":"user","credential":"secret"}]}`, true},
		{"IPv6", `{"iceServers":[{"urls":"stun:[::1]:3478"}]}`, true},
		{"empty", `{"iceServers":[]}`, false},
		{"null entry", `{"iceServers":[null]}`, false},
		{"empty object", `{"iceServers":[{}]}`, false},
		{"scalar entry", `{"iceServers":[7]}`, false},
		{"null URLs", `{"iceServers":[{"urls":null}]}`, false},
		{"mixed URLs", `{"iceServers":[{"urls":["stun:example.com",3]}]}`, false},
		{"missing username", `{"iceServers":[{"urls":"turn:example.com","credential":"secret"}]}`, false},
		{"missing credential", `{"iceServers":[{"urls":"turns:example.com","username":"user"}]}`, false},
		{"numeric credential", `{"iceServers":[{"urls":"turn:example.com","username":"user","credential":3}]}`, false},
		{"unsupported scheme", `{"iceServers":[{"urls":"https://example.com"}]}`, false},
		{"invalid port", `{"iceServers":[{"urls":"stun:example.com:70000"}]}`, false},
		{"invalid IPv6", `{"iceServers":[{"urls":"stun:[::::]:3478"}]}`, false},
		{"only blocked port", `{"iceServers":[{"urls":"stun:example.com:53"}]}`, false},
		{"trailing JSON", `{"iceServers":[{"urls":"stun:example.com"}]}{}`, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := parseAndValidateICE([]byte(tc.raw))
			if (err == nil) != tc.valid {
				t.Fatalf("valid=%v, err=%v", tc.valid, err)
			}
		})
	}
}

func TestICEFiltersBlockedPortAndNormalizes(t *testing.T) {
	raw := `{"iceServers":[{"urls":["stun:example.com:53","stun:example.com:3478"]},{"urls":"turns:example.com:443?transport=tcp","username":"user","credential":"secret"}]}`
	result, err := parseAndValidateICE([]byte(raw))
	if err != nil {
		t.Fatal(err)
	}
	var servers []iceServer
	if err := json.Unmarshal(result, &servers); err != nil {
		t.Fatal(err)
	}
	if len(servers) != 2 || len(servers[0].URLs) != 1 || servers[0].URLs[0] != "stun:example.com:3478" || servers[1].Credential != "secret" {
		t.Fatalf("unexpected normalization: %s", result)
	}
}
