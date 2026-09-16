#!/bin/sh
# Fake `cursor-agent acp` for zeron-harness tests: the ACP handshake whose
# session/new advertises the CLI's real catalog shape — configOptions model
# values composed as `base[param=value,…]`, including the `default[]` Auto
# alias row (verified live against cursor-agent's acp server).
emit() { printf '%s\n' "$1"; }
rid() { printf '%s' "$1" | sed 's/.*"id":\([0-9]*\).*/\1/'; }
has() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

read -r line || exit 1
has "$line" '"method":"initialize"' || exit 1
has "$line" '"protocolVersion":1' || exit 1
has "$line" '"name":"zeron"' || exit 1
emit "{\"id\":$(rid "$line"),\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}"

read -r line || exit 1
has "$line" '"method":"session/new"' || exit 1
has "$line" '"mcpServers":[]' || exit 1
emit "{\"id\":$(rid "$line"),\"result\":{\"sessionId\":\"s-1\",\"configOptions\":[{\"id\":\"model\",\"name\":\"Model\",\"category\":\"model\",\"type\":\"select\",\"currentValue\":\"default[]\",\"options\":[{\"value\":\"default[]\",\"name\":\"Auto\"},{\"value\":\"grok-4.6[effort=high,fast=true]\",\"name\":\"grok-4.6\"},{\"value\":\"claude-opus-5[thinking=true,context=300k,effort=high,fast=false]\",\"name\":\"claude-opus-5\"}]}],\"models\":{\"currentModelId\":\"default[]\",\"availableModels\":[{\"modelId\":\"default[]\",\"name\":\"Auto\"},{\"modelId\":\"grok-4.6[effort=high,fast=true]\",\"name\":\"grok-4.6\"},{\"modelId\":\"claude-opus-5[thinking=true,context=300k,effort=high,fast=false]\",\"name\":\"claude-opus-5\"}]}}}"

# Park until killed — the probe only needs the session/new response.
read -r _ || exit 0
