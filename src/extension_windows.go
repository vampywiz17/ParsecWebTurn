//go:build windows

package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func loadIceFallback(root string) (json.RawMessage, error) {
	raw, err := os.ReadFile(filepath.Join(root, "ice.json"))
	if err != nil {
		return nil, err
	}
	return parseAndValidateICE(raw)
}

func generateInjection(root string, servers json.RawMessage) error {
	// Validate here too so every caller has the same safety guarantees.
	normalized, err := parseAndValidateICE(append(append([]byte(`{"iceServers":`), servers...), '}'))
	if err != nil {
		return err
	}
	templatePath := filepath.Join(root, "extension", "inject.template.js")
	template, err := os.ReadFile(templatePath)
	if err != nil {
		return fmt.Errorf("cannot read extension/inject.template.js: %w", err)
	}
	if strings.Count(string(template), "__ICE_SERVERS__") != 1 {
		return fmt.Errorf("injection template must contain exactly one __ICE_SERVERS__ placeholder")
	}
	inject := strings.Replace(string(template), "__ICE_SERVERS__", string(normalized), 1)
	if err := atomicWriteFile(filepath.Join(root, "extension", "inject.js"), []byte(inject)); err != nil {
		return fmt.Errorf("cannot generate extension/inject.js: %w", err)
	}
	return nil
}
