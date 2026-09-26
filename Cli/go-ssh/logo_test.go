package main

import (
	"strings"
	"testing"
)

func TestGalaxyMatchesZcli(t *testing.T) {
	// rust-tui/crates/ztui/src/logo.rs and zdev/src/logo.ts at 12×4
	want := []string{
		"    ⢀⣄⣤⣤⣥⣥⣄⡀",
		"⣴⡃ ⣴⣿⣯⣽⣬⣍⠙⠻⣷",
		"⢿⣷⣤⣙⣻⣛⣻⣿⡿⠁⢀⠚",
		"⠈⠙⠛⠿⠛⠛⠋⠉    ",
	}
	got := logoGridDefault()
	if len(got) != 4 {
		t.Fatalf("rows %d", len(got))
	}
	for i, row := range got {
		s := logoRowPlain(row)
		if s != want[i] {
			t.Errorf("row %d\n want %q\n got  %q", i, want[i], s)
		}
	}
}

func TestGalaxy24x8HasFamilyMark(t *testing.T) {
	g := logoGrid(24, 8)
	var b strings.Builder
	for _, row := range g {
		b.WriteString(logoRowPlain(row))
	}
	if !strings.Contains(b.String(), "⣾⣿⣿⣿⠻⣟⣍⣋") {
		t.Fatalf("24×8 missing the zcli/zdev core mark; got %q", b.String())
	}
}

func TestLogoShade(t *testing.T) {
	if s := logoShade(1.2); s.color != "#f4f4f4" || !s.bold {
		t.Fatalf("core %v", s)
	}
	if s := logoShade(0.7); s.color != "#c4c4c4" || s.bold {
		t.Fatalf("arm %v", s)
	}
	if s := logoShade(0.2); s.color != "#7c7c7c" {
		t.Fatalf("dust %v", s)
	}
}

func TestTitleArtFits(t *testing.T) {
	a := makeTitleArt(80, 14)
	if a.rows == 0 || a.cols == 0 || len(a.cells) == 0 {
		t.Fatalf("empty title art %+v", a)
	}
	for _, c := range a.cells {
		if c[0] >= a.cols || c[1] >= a.rows || c[2] < 0x2800 {
			t.Fatalf("bad cell %v", c)
		}
	}
	if a2 := makeTitleArt(200, 40); a2.rows != 6 {
		t.Fatalf("wide title rows %d", a2.rows)
	}
}
