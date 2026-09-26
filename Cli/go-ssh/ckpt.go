package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

func ckptDir(home, session string) string {
	return filepath.Join(home, "ckpt", session)
}

func ckptSave(home, session, file string) string {
	st, err := os.Stat(file)
	if err != nil || !st.Mode().IsRegular() {
		return ""
	}
	dir := ckptDir(home, session)
	os.MkdirAll(dir, 0o755)
	seq := 0
	ents, _ := os.ReadDir(dir)
	for _, e := range ents {
		n, _, _ := strings.Cut(e.Name(), "-")
		if i, err := strconv.Atoi(n); err == nil && i > seq {
			seq = i
		}
	}
	seq++
	base := filepath.Base(file)
	dest := filepath.Join(dir, fmt.Sprintf("%d-%s", seq, base))
	data, err := os.ReadFile(file)
	if err != nil {
		return ""
	}
	os.WriteFile(dest, data, 0o644)
	os.WriteFile(filepath.Join(dir, fmt.Sprintf("%d.meta", seq)), []byte(file), 0o644)
	return dest
}

func ckptRestoreLast(home, session string) (string, error) {
	dir := ckptDir(home, session)
	ents, err := os.ReadDir(dir)
	if err != nil {
		return "", fmt.Errorf("no checkpoints")
	}
	type meta struct {
		seq int
		p   string
	}
	var metas []meta
	for _, e := range ents {
		if strings.HasSuffix(e.Name(), ".meta") {
			n := strings.TrimSuffix(e.Name(), ".meta")
			if i, err := strconv.Atoi(n); err == nil {
				metas = append(metas, meta{i, filepath.Join(dir, e.Name())})
			}
		}
	}
	if len(metas) == 0 {
		return "", fmt.Errorf("no checkpoints")
	}
	last := metas[0]
	for _, m := range metas[1:] {
		if m.seq > last.seq {
			last = m
		}
	}
	targetB, err := os.ReadFile(last.p)
	if err != nil {
		return "", err
	}
	target := string(targetB)
	src := filepath.Join(dir, fmt.Sprintf("%d-%s", last.seq, filepath.Base(target)))
	os.MkdirAll(filepath.Dir(target), 0o755)
	data, err := os.ReadFile(src)
	if err != nil {
		return "", fmt.Errorf("checkpoint blob missing: %s", src)
	}
	os.WriteFile(target, data, 0o644)
	os.Remove(src)
	os.Remove(last.p)
	return target, nil
}
