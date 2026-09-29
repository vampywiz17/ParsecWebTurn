//go:build windows

package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// Only change the dedicated app profile while Edge is stopped. Preserve all
// unrelated preferences, including JSON numbers without float64 rounding.
func prepareEdgeProfile(profile string) error {
	if err := ensureProfileStopped(profile); err != nil {
		return err
	}
	directory := filepath.Join(profile, "Default")
	if err := os.MkdirAll(directory, 0700); err != nil {
		return err
	}
	path := filepath.Join(directory, "Preferences")
	preferences := make(map[string]json.RawMessage)
	data, err := os.ReadFile(path)
	if err == nil {
		if err := json.Unmarshal(data, &preferences); err != nil || preferences == nil {
			return errors.New("cannot read Edge preferences; existing profile was left unchanged")
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("cannot read Edge preferences: %w", err)
	}
	translate := make(map[string]json.RawMessage)
	if raw, ok := preferences["translate"]; ok {
		if err := json.Unmarshal(raw, &translate); err != nil || translate == nil {
			return errors.New("cannot read Edge translation preferences; existing profile was left unchanged")
		}
	}
	// Chromium's kOfferTranslateEnabled suppresses the automatic translate bubble.
	translate["enabled"] = json.RawMessage("false")
	preferences["translate"], err = json.Marshal(translate)
	if err != nil {
		return err
	}
	data, err = json.Marshal(preferences)
	if err != nil {
		return err
	}
	return atomicWriteFile(path, data)
}
