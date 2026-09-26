//go:build windows

package main

import (
	"os"
	"syscall"
	"unicode/utf16"
	"unsafe"
)

const (
	credTypeGeneric       = 1
	credPersistEnterprise = 3
	wincredService        = "cosmos-cli"
)

var (
	modAdvapi32     = syscall.NewLazyDLL("advapi32.dll")
	procCredReadW   = modAdvapi32.NewProc("CredReadW")
	procCredWriteW  = modAdvapi32.NewProc("CredWriteW")
	procCredDeleteW = modAdvapi32.NewProc("CredDeleteW")
	procCredFree    = modAdvapi32.NewProc("CredFree")
)

func wincredTarget(id string) string { return id + "." + wincredService }

func keyringEnabled() bool {
	return os.Getenv("COSMOS_CLI_KEYRING") != "0"
}

func wincredRead(id string) string {
	if !keyringEnabled() {
		return ""
	}
	t, err := syscall.UTF16PtrFromString(wincredTarget(id))
	if err != nil {
		return ""
	}
	var out uintptr
	r, _, _ := procCredReadW.Call(uintptr(unsafe.Pointer(t)), credTypeGeneric, 0, uintptr(unsafe.Pointer(&out)))
	if r == 0 || out == 0 {
		return ""
	}
	defer procCredFree.Call(out)
	blobSize := *(*uint32)(unsafe.Pointer(out + 32))
	blob := *(*uintptr)(unsafe.Pointer(out + 40))
	if blob == 0 || blobSize == 0 || blobSize%2 != 0 {
		return ""
	}
	n := int(blobSize / 2)
	u16 := unsafe.Slice((*uint16)(unsafe.Pointer(blob)), n)
	return string(utf16.Decode(u16))
}

func wincredWrite(id, key string) bool {
	if !keyringEnabled() {
		return false
	}
	t, err := syscall.UTF16PtrFromString(wincredTarget(id))
	if err != nil {
		return false
	}
	user, err := syscall.UTF16PtrFromString(id)
	if err != nil {
		return false
	}
	comment, _ := syscall.UTF16PtrFromString("keyring v3.6.3")
	blob := utf16.Encode([]rune(key))
	blobBytes := unsafe.Slice((*byte)(unsafe.Pointer(&blob[0])), len(blob)*2)
	type credW struct {
		flags, typ uint32
		target     *uint16
		comment    *uint16
		lastWrite  [8]byte
		blobSize   uint32
		_pad       uint32
		blob       *byte
		persist    uint32
		attrCount  uint32
		attrs      uintptr
		alias      *uint16
		user       *uint16
	}
	c := credW{
		typ:      credTypeGeneric,
		target:   t,
		comment:  comment,
		blobSize: uint32(len(blobBytes)),
		blob:     &blobBytes[0],
		persist:  credPersistEnterprise,
		user:     user,
	}
	r, _, _ := procCredWriteW.Call(uintptr(unsafe.Pointer(&c)), 0)
	return r != 0
}

func wincredRemove(id string) {
	if !keyringEnabled() {
		return
	}
	t, err := syscall.UTF16PtrFromString(wincredTarget(id))
	if err != nil {
		return
	}
	procCredDeleteW.Call(uintptr(unsafe.Pointer(t)), credTypeGeneric, 0)
}
