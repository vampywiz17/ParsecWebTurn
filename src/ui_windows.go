//go:build windows

package main

import (
	"github.com/lxn/walk"
	. "github.com/lxn/walk/declarative"
)

func createSettingsWindow(root string, current Settings, currentToken string) (*walk.MainWindow, *bool, error) {
	var mw *walk.MainWindow
	var keyEdit, tokenEdit *walk.LineEdit
	var ttlEdit *walk.NumberEdit
	var cacheCheck *walk.CheckBox
	saved := false

	ttl := current.TTL
	if ttl < 60 || ttl > maxTTL {
		ttl = defaultTTL
	}

	err := (MainWindow{
		AssignTo: &mw,
		Title:    "ParsecWebTurn - Cloudflare TURN Settings",
		MinSize:  Size{Width: 560, Height: 360},
		Size:     Size{Width: 600, Height: 400},
		Layout:   VBox{MarginsZero: false},
		Children: []Widget{
			Label{Text: "Cloudflare Realtime TURN credentials"},
			Label{Text: "TURN Key ID:"},
			LineEdit{
				AssignTo: &keyEdit,
				Text:     current.TurnKeyID,
			},
			Label{Text: "TURN Key API Token (stored encrypted with Windows DPAPI):"},
			LineEdit{
				AssignTo:     &tokenEdit,
				Text:         currentToken,
				PasswordMode: true,
			},
			Label{Text: "Generated credential lifetime (seconds, max 172800 / 48h):"},
			NumberEdit{
				AssignTo: &ttlEdit,
				MinValue: 60,
				MaxValue: maxTTL,
				Value:    float64(ttl),
				Decimals: 0,
			},
			Label{
				Text: "The API token stays on this PC. ParsecWebTurn uses it only to request short-lived TURN credentials directly from Cloudflare.",
			},
			CheckBox{AssignTo: &cacheCheck, Text: "Reuse valid TURN credentials for faster startup", Checked: current.CacheCredentials},
			Composite{
				Layout: HBox{},
				Children: []Widget{
					HSpacer{},
					PushButton{
						Text: "Save && Start",
						OnClicked: func() {
							if err := saveSettings(root, keyEdit.Text(), tokenEdit.Text(), int(ttlEdit.Value()), cacheCheck.Checked()); err != nil {
								walk.MsgBox(mw, "ParsecWebTurn", err.Error(), walk.MsgBoxIconError)
								return
							}
							saved = true
							mw.Close()
						},
					},
					PushButton{
						Text: "Cancel",
						OnClicked: func() {
							mw.Close()
						},
					},
				},
			},
		},
	}).Create()
	return mw, &saved, err
}

func showSettings(root string, current Settings, currentToken string) bool {
	mw, saved, err := createSettingsWindow(root, current, currentToken)
	if err != nil {
		walk.MsgBox(nil, "ParsecWebTurn", "Cannot create settings window:\n"+err.Error(), walk.MsgBoxIconError)
		return false
	}
	defer mw.Dispose()
	mw.Run()
	return *saved
}
