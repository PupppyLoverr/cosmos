// The Cosmos mark — the same black-and-white Milky Way zcli draws
// (rust-tui/crates/ztui/src/logo.rs) and zdev ports (src/logo.ts), number
// for number: a tilted two-arm logarithmic spiral with a bright core and a
// few field stars, sampled on a 2×4-dots-per-cell braille grid and shaded
// by the light each cell holds. At 12×4 it is identical to zcli; larger
// sizes sample the same galaxy more finely. Title dust ("welcome to cosmos")
// matches zcli-welcome / zdev welcome.
package main

import (
	"fmt"
	"math"
	"sync"
)

const (
	logoCols = 12
	logoRows = 4
)

type logoCell struct {
	ch  rune
	lum float64
}

type logoShadeStyle struct {
	color string
	bold  bool
}

// noise is the cheap integer hash → [0, 1) from logo.rs, bit-identical.
func logoNoise(x, y int, salt uint32) float64 {
	h := uint32(x)*0x9E3779B1 ^ uint32(y)*0x85EBCA77 ^ salt
	h ^= h >> 15
	h *= 0x2C1B3C6D
	h ^= h >> 12
	h *= 0x297A2D39
	h ^= h >> 15
	return float64(h&0xFFFF) / 65536.0
}

// logoLight is the light at one dot of a W×H grid (0 = empty sky).
func logoLight(x, y, W, H int) float64 {
	cx := (float64(W) - 1) / 2
	cy := (float64(H) - 1) / 2
	dx := (float64(x) - cx) / (float64(W) / 2)
	dy := (float64(y) - cy) / (float64(H) / 2)
	a := -0.4
	u := dx*math.Cos(a) + dy*math.Sin(a)
	v := (-dx*math.Sin(a) + dy*math.Cos(a)) / 0.85
	r := math.Sqrt(u*u + v*v)
	theta := math.Atan2(v, u)
	core := math.Exp(-math.Pow(r/0.25, 2)) * 1.6
	wind := theta - 3*math.Log(r+0.05)
	arm := math.Pow((1+math.Cos(2*wind))/2, 1.5)
	disk := 0.0
	if r < 1.05 {
		disk = math.Exp(-(r / 0.9))
	}
	dust := arm*disk*2.2 + disk*0.18
	stars := 0.0
	if logoNoise(x, y, 7) > 0.975 && r > 0.8 {
		stars = 0.8
	}
	return core + dust + stars
}

var logoBIT = [2][4]uint32{
	{0x01, 0x02, 0x04, 0x40},
	{0x08, 0x10, 0x20, 0x80},
}

var (
	logoCache   = map[string][][]logoCell{}
	logoCacheMu sync.Mutex
)

// logoGrid returns the galaxy as rows × cols braille cells.
func logoGrid(cols, rows int) [][]logoCell {
	if cols <= 0 {
		cols = logoCols
	}
	if rows <= 0 {
		rows = logoRows
	}
	key := fmt.Sprintf("%dx%d", cols, rows)
	logoCacheMu.Lock()
	if hit, ok := logoCache[key]; ok {
		logoCacheMu.Unlock()
		return hit
	}
	logoCacheMu.Unlock()

	W := cols * 2
	H := rows * 4
	out := make([][]logoCell, rows)
	for cy := 0; cy < rows; cy++ {
		row := make([]logoCell, cols)
		for cx := 0; cx < cols; cx++ {
			var bits uint32
			var sum float64
			var on uint32
			for ix := 0; ix < 2; ix++ {
				for iy := 0; iy < 4; iy++ {
					x := cx*2 + ix
					y := cy*4 + iy
					l := logoLight(x, y, W, H)
					if l+(logoNoise(x, y, 31)-0.5)*0.35 > 0.34 {
						bits |= logoBIT[ix][iy]
						sum += l
						on++
					}
				}
			}
			ch := rune(' ')
			if bits != 0 {
				ch = rune(0x2800 + bits)
			}
			lum := 0.0
			if on > 0 {
				lum = sum / float64(on)
			}
			row[cx] = logoCell{ch: ch, lum: lum}
		}
		out[cy] = row
	}
	logoCacheMu.Lock()
	logoCache[key] = out
	logoCacheMu.Unlock()
	return out
}

func logoGridDefault() [][]logoCell { return logoGrid(logoCols, logoRows) }

func logoShade(lum float64) logoShadeStyle {
	if lum > 1.0 {
		return logoShadeStyle{color: "#f4f4f4", bold: true}
	}
	if lum > 0.55 {
		return logoShadeStyle{color: "#c4c4c4"}
	}
	return logoShadeStyle{color: "#7c7c7c"}
}

func logoRowPlain(row []logoCell) string {
	b := make([]rune, len(row))
	for i, c := range row {
		b[i] = c.ch
	}
	return string(b)
}

// ── "welcome to cosmos" in star dust ────────────────────────────────────────

// 5×8 bitmaps: a 2-row ascender and a 6-row x-height, so at 2× every pair
// of pixel rows fills exactly one braille cell.
var titleGlyph = map[rune][]string{
	'c': {".....", ".....", ".###.", "#...#", "#....", "#....", "#...#", ".###."},
	'e': {".....", ".....", ".###.", "#...#", "#####", "#....", "#...#", ".###."},
	'l': {".##..", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."},
	'm': {".....", ".....", "##.#.", "#.#.#", "#.#.#", "#.#.#", "#.#.#", "#.#.#"},
	'o': {".....", ".....", ".###.", "#...#", "#...#", "#...#", "#...#", ".###."},
	's': {".....", ".....", ".###.", "#....", ".###.", "....#", "#...#", ".###."},
	't': {".#...", ".#...", "####.", ".#...", ".#...", ".#...", ".#..#", "..##."},
	'w': {".....", ".....", "#...#", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#."},
}

type titleArt struct {
	rows, cols int
	cells      [][3]int // c, r, rune
}

func titleLinePx(s string, k int) int {
	n := 0
	for _, c := range s {
		if c == ' ' {
			n += 3 * k
		} else {
			n += 6 * k
		}
	}
	if n > 0 {
		n -= k
	}
	if n < 0 {
		return 0
	}
	return n
}

// titleArt renders "welcome to cosmos" as braille cells; picks the biggest
// layout that fits, matching zdev/zwelcome.
func makeTitleArt(w, h int) titleArt {
	type layout struct {
		lines []string
		k     int
	}
	layouts := []layout{
		{[]string{"welcome to cosmos"}, 3},
		{[]string{"welcome to cosmos"}, 2},
		{[]string{"welcome to", "cosmos"}, 2},
		{[]string{"welcome to cosmos"}, 1},
	}
	pick := layouts[len(layouts)-1]
	for _, l := range layouts {
		wpx := 0
		for _, s := range l.lines {
			if p := titleLinePx(s, l.k); p > wpx {
				wpx = p
			}
		}
		hpx := len(l.lines)*10*l.k - 2*l.k
		if int(math.Ceil(float64(wpx)/2))+2 <= w && int(math.Ceil(float64(hpx)/4)) <= h {
			pick = l
			break
		}
	}
	k := pick.k
	wpx := 0
	for _, s := range pick.lines {
		if p := titleLinePx(s, k); p > wpx {
			wpx = p
		}
	}
	hpx := len(pick.lines)*10*k - 2*k
	if wpx < 1 {
		wpx = 1
	}
	if hpx < 1 {
		hpx = 1
	}
	dots := make([][]bool, hpx)
	for i := range dots {
		dots[i] = make([]bool, wpx)
	}
	for li, s := range pick.lines {
		x := (wpx - titleLinePx(s, k)) / 2
		x &^= 1
		y0 := li * 10 * k
		for _, c := range s {
			if c == ' ' {
				x += 3 * k
				continue
			}
			g := titleGlyph[c]
			for gy, row := range g {
				for gx, b := range row {
					if b != '#' {
						continue
					}
					for dy := 0; dy < k; dy++ {
						for dx := 0; dx < k; dx++ {
							yy := y0 + gy*k + dy
							xx := x + gx*k + dx
							if yy >= 0 && yy < hpx && xx >= 0 && xx < wpx {
								dots[yy][xx] = true
							}
						}
					}
				}
			}
			x += 6 * k
		}
	}
	rows := int(math.Ceil(float64(hpx) / 4))
	cols := int(math.Ceil(float64(wpx) / 2))
	bits := [2][4]int{{0x01, 0x02, 0x04, 0x40}, {0x08, 0x10, 0x20, 0x80}}
	var cells [][3]int
	for r := 0; r < rows; r++ {
		for c := 0; c < cols; c++ {
			b := 0
			for dx := 0; dx < 2; dx++ {
				for dy := 0; dy < 4; dy++ {
					yy := r*4 + dy
					xx := c*2 + dx
					if yy < hpx && xx < wpx && dots[yy][xx] {
						b |= bits[dx][dy]
					}
				}
			}
			if b != 0 {
				cells = append(cells, [3]int{c, r, 0x2800 + b})
			}
		}
	}
	return titleArt{rows: rows, cols: cols, cells: cells}
}
