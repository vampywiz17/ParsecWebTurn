//go:build windows

package main

import (
	"strings"
	"syscall"
	"unsafe"

	"github.com/lxn/walk"
	. "github.com/lxn/walk/declarative"
)

func createSettingsWindow(root string, current Settings, currentSecret string) (*walk.MainWindow, *bool, error) {
	var mw *walk.MainWindow
	var provider *walk.ComboBox
	var cloudPanel, customPanel *walk.Composite
	var keyEdit, tokenEdit, userEdit, passwordEdit *walk.LineEdit
	var urlsEdit *walk.TextEdit
	var ttlEdit *walk.NumberEdit
	var cacheCheck *walk.CheckBox
	var errorLabel *walk.TextLabel
	saved := false
	cloudToken, _ := unprotectString(current.EncryptedToken)
	customPassword, _ := unprotectString(current.EncryptedCustomPassword)
	providerIndex := 0
	if current.Provider == providerCustom {
		providerIndex = 1
		customPassword = currentSecret
	} else {
		cloudToken = currentSecret
	}
	ttl := current.TTL
	if ttl < 60 || ttl > maxTTL {
		ttl = defaultTTL
	}

	background := walk.RGB(22, 23, 29)
	card := walk.RGB(247, 248, 250)
	ink := walk.RGB(35, 38, 48)
	muted := walk.RGB(104, 111, 128)
	accent := walk.RGB(239, 75, 105)
	label := func(text string) Label {
		return Label{Text: text, TextColor: ink, Font: Font{PointSize: 10, Bold: true}}
	}
	help := func(text string) TextLabel {
		return TextLabel{Text: text, TextColor: muted, MinSize: Size{Width: 500}, Font: Font{PointSize: 9}}
	}
	updateProvider := func() {
		if provider == nil || cloudPanel == nil || customPanel == nil {
			return
		}
		custom := provider.CurrentIndex() == 1
		cloudPanel.SetVisible(!custom)
		customPanel.SetVisible(custom)
		if errorLabel != nil {
			errorLabel.SetVisible(false)
		}
	}

	err := (MainWindow{
		AssignTo: &mw, Title: "ParsecWebTurn - Connection Settings", SuspendedUntilRun: true,
		Font: Font{Family: "Segoe UI", PointSize: 10}, Background: SolidColorBrush{Color: background},
		MinSize: Size{Width: 700, Height: 600}, Size: Size{Width: 760, Height: 650},
		Layout: VBox{Margins: Margins{Left: 28, Top: 24, Right: 28, Bottom: 24}, Spacing: 18},
		Children: []Widget{
			Label{Text: "PARSEC WEB TURN", TextColor: accent, Font: Font{PointSize: 11, Bold: true}},
			Label{Text: "Connection settings", TextColor: walk.RGB(248, 249, 252), Font: Font{PointSize: 24, Bold: true}},
			Label{Text: "Choose your TURN provider. Keep your connection in your control.", TextColor: walk.RGB(173, 178, 193)},
			Composite{
				Background: SolidColorBrush{Color: card}, StretchFactor: 1,
				Layout: VBox{Margins: Margins{Left: 24, Top: 22, Right: 24, Bottom: 22}, Spacing: 12},
				Children: []Widget{
					label("TURN PROVIDER"),
					ComboBox{Name: "provider", AssignTo: &provider, Model: []string{"Cloudflare Realtime", "Custom server  /  coturn, eturnal"}, CurrentIndex: providerIndex, MinSize: Size{Height: 32}, OnCurrentIndexChanged: updateProvider},
					Composite{Name: "cloudflare", AssignTo: &cloudPanel, Visible: providerIndex == 0, Layout: VBox{MarginsZero: true, Spacing: 10}, Children: []Widget{
						help("Generate temporary TURN credentials automatically using your Cloudflare key."),
						label("Key ID"),
						LineEdit{AssignTo: &keyEdit, Text: current.TurnKeyID, CueBanner: "Cloudflare TURN Key ID", MinSize: Size{Height: 32}},
						label("API token"),
						LineEdit{AssignTo: &tokenEdit, Text: cloudToken, PasswordMode: true, CueBanner: "TURN Key API token", MinSize: Size{Height: 32}},
						Composite{Layout: Grid{Columns: 2, MarginsZero: true, Spacing: 12}, Children: []Widget{
							Label{Row: 0, Column: 0, Text: "Credential lifetime (seconds)", TextColor: ink},
							NumberEdit{Row: 0, Column: 1, AssignTo: &ttlEdit, MinValue: 60, MaxValue: maxTTL, Value: float64(ttl), Decimals: 0, MinSize: Size{Width: 150, Height: 32}},
						}},
						CheckBox{AssignTo: &cacheCheck, Text: "Reuse valid credentials for faster startup", Checked: current.CacheCredentials},
						help("Maximum lifetime: 48 hours. Cached credentials may have less time remaining."),
					}},
					Composite{Name: "custom", AssignTo: &customPanel, Visible: providerIndex == 1, Layout: VBox{MarginsZero: true, Spacing: 10}, Children: []Widget{
						help("Use your own TURN service with static or externally generated credentials."),
						label("STUN / TURN URLs"),
						TextEdit{Name: "urls", AssignTo: &urlsEdit, Text: strings.Join(current.CustomURLs, "\r\n"), VScroll: true, MinSize: Size{Height: 90}, MaxSize: Size{Height: 110}, Font: Font{Family: "Consolas", PointSize: 10}},
						help("One URL per line. Example: turns:turn.example.com:5349?transport=tcp"),
						label("Username"),
						LineEdit{Name: "username", AssignTo: &userEdit, Text: current.CustomUsername, CueBanner: "TURN username", MinSize: Size{Height: 32}},
						label("Password / credential"),
						LineEdit{Name: "password", AssignTo: &passwordEdit, Text: customPassword, PasswordMode: true, CueBanner: "TURN password", MinSize: Size{Height: 32}},
						help("All TURN URLs use these credentials. STUN-only setups can leave them empty. A TURN shared secret is not a client password."),
					}},
					TextLabel{Name: "error", AssignTo: &errorLabel, TextColor: walk.RGB(180, 35, 65), MinSize: Size{Width: 500}, Visible: false},
				},
			},
			Composite{Background: SolidColorBrush{Color: background}, Layout: HBox{MarginsZero: true, Spacing: 12}, Children: []Widget{
				Label{Text: "Saved secrets encrypted for your Windows account", TextColor: walk.RGB(173, 178, 193), Font: Font{PointSize: 9}},
				HSpacer{},
				PushButton{Text: "Cancel", MinSize: Size{Width: 85, Height: 36}, OnClicked: func() { mw.Close() }},
				PushButton{Name: "save", Text: "Save && launch Parsec", Font: Font{PointSize: 10, Bold: true}, MinSize: Size{Width: 170, Height: 36}, OnClicked: func() {
					s := current
					s.Provider = providerCloudflare
					if provider.CurrentIndex() == 1 {
						s.Provider = providerCustom
					}
					s.TurnKeyID, s.TTL, s.CacheCredentials = keyEdit.Text(), int(ttlEdit.Value()), cacheCheck.Checked()
					s.CustomURLs = strings.Split(strings.ReplaceAll(urlsEdit.Text(), "\r\n", "\n"), "\n")
					s.CustomUsername = userEdit.Text()
					if err := saveConfiguration(root, s, tokenEdit.Text(), passwordEdit.Text()); err != nil {
						errorLabel.SetText(err.Error())
						errorLabel.SetVisible(true)
						return
					}
					saved = true
					mw.Close()
				}},
			}},
		},
	}).Create()
	if err == nil {
		updateProvider()
		// Older Windows versions can ignore this optional dark title bar hint.
		dark := int32(1)
		syscall.NewLazyDLL("dwmapi.dll").NewProc("DwmSetWindowAttribute").Call(uintptr(mw.Handle()), 20, uintptr(unsafe.Pointer(&dark)), unsafe.Sizeof(dark))
	}
	return mw, &saved, err
}

func showSettings(root string, current Settings, currentSecret string) bool {
	mw, saved, err := createSettingsWindow(root, current, currentSecret)
	if err != nil {
		walk.MsgBox(nil, "ParsecWebTurn", "Cannot create settings window:\n"+err.Error(), walk.MsgBoxIconError)
		return false
	}
	defer mw.Dispose()
	mw.Run()
	return *saved
}
