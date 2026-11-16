// Run inside the patched router module: copy to cmd/audit-capture/main.go,
// then go run ./cmd/audit-capture <evidence-output-directory>.
// All traffic terminates on loopback; no credentials or remote sites are used.
package main

import (
	"bufio"
	"context"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v8/internal/codextls"
	ag "github.com/router-for-me/CLIProxyAPI/v8/internal/translator/antigravity/claude"
	codex "github.com/router-for-me/CLIProxyAPI/v8/internal/translator/codex/claude"
)

type capture struct {
	Route      string   `json:"route"`
	Connect    string   `json:"connect"`
	RecordHex  string   `json:"record_hex"`
	Ciphers    []uint16 `json:"ciphers"`
	Extensions []uint16 `json:"extensions"`
	Error      string   `json:"expected_client_error"`
}

func hello(viaProxy bool) capture {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	defer ln.Close()
	result := make(chan capture, 1)
	go func() {
		conn, e := ln.Accept()
		if e != nil {
			panic(e)
		}
		defer conn.Close()
		_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
		r := bufio.NewReader(conn)
		c := capture{Route: "direct"}
		if viaProxy {
			c.Route = "http-connect"
			req, e := http.ReadRequest(r)
			if e != nil {
				panic(e)
			}
			c.Connect = req.Method + " " + req.Host
			if req.Method != "CONNECT" {
				panic(c.Connect)
			}
			_, _ = io.WriteString(conn, "HTTP/1.1 200 Connection Established\r\n\r\n")
		}
		hdr := make([]byte, 5)
		if _, e := io.ReadFull(r, hdr); e != nil {
			panic(e)
		}
		body := make([]byte, int(binary.BigEndian.Uint16(hdr[3:])))
		if _, e := io.ReadFull(r, body); e != nil {
			panic(e)
		}
		c.RecordHex = hex.EncodeToString(append(hdr, body...))
		// TLS record -> handshake -> legacy version/random -> session ID -> ciphers.
		i := 4 + 2 + 32
		i += 1 + int(body[i])
		n := int(binary.BigEndian.Uint16(body[i:]))
		i += 2
		for end := i + n; i < end; i += 2 {
			c.Ciphers = append(c.Ciphers, binary.BigEndian.Uint16(body[i:]))
		}
		i += 1 + int(body[i])
		n = int(binary.BigEndian.Uint16(body[i:]))
		i += 2
		for end := i + n; i < end; {
			c.Extensions = append(c.Extensions, binary.BigEndian.Uint16(body[i:]))
			size := int(binary.BigEndian.Uint16(body[i+2:]))
			i += 4 + size
		}
		result <- c
	}()
	proxyURL := ""
	target := "https://" + ln.Addr().String() + "/capture"
	if viaProxy {
		proxyURL = "http://" + ln.Addr().String()
		target = "https://audit-target.test/capture"
	}
	rt := codextls.RoundTripper(fmt.Sprintf("capture-%t", viaProxy), proxyURL, 3*time.Second)
	defer rt.(*http.Transport).CloseIdleConnections()
	req, _ := http.NewRequestWithContext(context.Background(), "GET", target, nil)
	response, err := (&http.Client{Transport: rt, Timeout: 5 * time.Second}).Do(req)
	if response != nil {
		response.Body.Close()
	}
	c := <-result
	if err != nil {
		c.Error = err.Error()
	}
	return c
}

func main() {
	dir := os.Args[1]
	if err := os.MkdirAll(dir, 0755); err != nil {
		panic(err)
	}
	sentence := "You are Claude Code, Anthropic's official CLI for Claude."
	samples := []struct{ Name, Text string }{
		{"D1", "x-anthropic-billing-header: sample\nKeep project rules.\nRead CLAUDE.md."},
		{"D4", sentence + "\n`" + sentence + "`\n```text\n" + sentence + "\n```"},
		{"D6a", "由腾讯开发，支持小米手机、华为技术与字节跳动。\n`腾讯`\n```text\n由百度开发\n```\nOpenAIClient"},
	}
	var prompts []map[string]any
	for _, s := range samples {
		for _, shape := range []string{"string", "array"} {
			var sys any = s.Text
			if shape == "array" {
				sys = []any{map[string]any{"type": "text", "text": s.Text}}
			}
			payload, _ := json.Marshal(map[string]any{"system": sys, "messages": []any{map[string]any{"role": "user", "content": "hello"}}})
			c, e := codex.ConvertClaudeRequestToCodex("codex-test", payload, false)
			if e != nil {
				panic(e)
			}
			a, e := ag.ConvertClaudeRequestToAntigravity("claude-sonnet-4-5", payload, false)
			if e != nil {
				panic(e)
			}
			prompts = append(prompts, map[string]any{"finding": s.Name, "shape": shape, "input": json.RawMessage(payload), "codex": json.RawMessage(c), "antigravity": json.RawMessage(a)})
		}
	}
	for name, value := range map[string]any{"prompts.json": prompts, "tls.json": []capture{hello(false), hello(true)}} {
		data, e := json.MarshalIndent(value, "", "  ")
		if e != nil {
			panic(e)
		}
		if e = os.WriteFile(filepath.Join(dir, name), append(data, '\n'), 0644); e != nil {
			panic(e)
		}
	}
	fmt.Println("Captured real translator outputs and loopback ClientHello records in", dir)
}
