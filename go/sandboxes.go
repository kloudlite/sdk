package kloudlite

// Sandboxes: short-lived, isolated pods for running code, over /v1/sandboxes —
// Modal-shaped. Create returns at once; Exec starts a process and answers a
// *Process whose Stdin is an io.Writer and whose Stdout/Stderr follow the
// process's NDJSON stream until it exits:
//
//	sb, err := kl.Sandboxes.Create(kloudlite.CreateSandbox{Image: "python:3.12"})
//	p, err := sb.Exec(kloudlite.ExecCommand{Cmd: []string{"python", "-c", "print(input())"}})
//	p.Stdin.Write([]byte("hi\n"))
//	p.Stdin.WriteEOF()
//	code, err := p.Wait(0)
//	out, err := p.Stdout.ReadAll()
//	sb.Terminate()
//
// Every exec is DETACHED: the api clamps a job that waits to 25 s (its kube
// client bounds a call's headers), so the SDK starts a process and waits in
// short process_wait polls instead.

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// ErrTimeout is wrapped by every wait that ran out of time (errors.Is).
var ErrTimeout = errors.New("timed out")

// maxWaitSlice is the server's ceiling on one process_wait.
const maxWaitSlice = 20 * time.Second

// Sandboxes is the sandbox module: /v1/sandboxes.
type Sandboxes struct{ c *Client }

// CreateSandbox carries what a create takes; every zero field is left out and
// means the server's default (the platform image, 300 s, 1 core, 2 GB, 10 GB,
// network "internet"). FromWorkspace (an id, "name" or "team/name") or
// FromSnapshot (a snapshot id) seed the disk. Timeout is the sandbox's whole
// lifetime, IdleTimeout how long it may sit with nothing running; both travel
// in whole seconds.
type CreateSandbox struct {
	Image         string
	Packages      []string
	FromWorkspace string
	FromSnapshot  string
	Team          string
	Region        string
	Timeout       time.Duration
	IdleTimeout   time.Duration
	// Network is "internet" or "none".
	Network  string
	Env      map[string]string
	Cpu      int
	MemoryGb int
	DiskGb   int
	// Environment connects the sandbox to an environment its owner (Team,
	// else you) owns — an id, "name" or "owner/name": the environment's
	// services resolve by bare name inside it, and an idle-stopped one is
	// started.
	Environment string
}

// RunSandbox is Run's input: Cmd ("shell string" or []string argv) with Cwd
// and Env runs in a sandbox made from Sandbox. Timeout bounds the wait for the
// process (0 = until it exits); the sandbox is terminated either way.
type RunSandbox struct {
	Cmd     any
	Cwd     string
	Env     map[string]string
	Timeout time.Duration
	Sandbox CreateSandbox
}

// RunResult is what Run answers: the exit code (-1 when the process ended
// without one, e.g. by a signal), both streams whole, and the sandbox's id.
type RunResult struct {
	ExitCode int
	Stdout   string
	Stderr   string
	Sandbox  string
}

// ExecCommand starts Cmd — a shell string or an []string argv — in the
// sandbox; Cwd is relative to ~/workspace, Env adds to the sandbox's own.
type ExecCommand struct {
	Cmd any
	Cwd string
	Env map[string]string
}

// SandboxDoc is the server's view of a sandbox. Phase is pending, starting,
// running or terminated; Reason (once terminated) is Terminated, Timeout,
// Idle, Exited, Failed, Unplaceable, SourceGone, Lost or EnvironmentGone.
// Times are RFC 3339.
type SandboxDoc struct {
	ID              string            `json:"id"`
	Owner           string            `json:"owner"`
	Team            string            `json:"team"`
	Region          string            `json:"region"`
	Image           string            `json:"image"`
	Packages        []string          `json:"packages"`
	Phase           string            `json:"phase"`
	Reason          string            `json:"reason"`
	Message         string            `json:"message"`
	CreatedAt       string            `json:"created_at"`
	StartedAt       string            `json:"started_at"`
	FinishedAt      string            `json:"finished_at"`
	TimeoutSecs     int64             `json:"timeout_secs"`
	IdleTimeoutSecs int64             `json:"idle_timeout_secs"`
	Network         string            `json:"network"`
	Cpu             int64             `json:"cpu"`
	MemoryGb        int64             `json:"memory_gb"`
	DiskGb          int64             `json:"disk_gb"`
	From            map[string]string `json:"from"`
	Environment     string            `json:"environment"`
}

// Sandbox is a handle on one sandbox; its doc is the last one read (Refresh
// re-reads it). A handle is not safe for concurrent Refresh calls; the
// Processes it hands out are independent of it.
type Sandbox struct {
	SandboxDoc
	c *Client
}

func secs(d time.Duration) int64 {
	if d <= 0 {
		return 0
	}
	return int64((d + time.Second - 1) / time.Second)
}

// Create asks for a sandbox and returns at once (202) — it does not wait for
// it to run; Exec does, or WaitRunning.
func (s *Sandboxes) Create(req CreateSandbox) (*Sandbox, error) {
	a := toolArgs{}.str("team", req.Team).str("region", req.Region).str("image", req.Image).
		val("packages", req.Packages, req.Packages != nil).
		str("from_workspace", req.FromWorkspace).str("from_snapshot", req.FromSnapshot).
		num("timeout_secs", secs(req.Timeout)).num("idle_timeout_secs", secs(req.IdleTimeout)).
		str("network", req.Network).val("env", req.Env, req.Env != nil).
		num("cpu", int64(req.Cpu)).num("memory_gb", int64(req.MemoryGb)).num("disk_gb", int64(req.DiskGb)).
		str("environment", req.Environment)
	sb := &Sandbox{c: s.c}
	if err := s.c.call("POST", "/v1/sandboxes", nil, map[string]any(a), &sb.SandboxDoc); err != nil {
		return nil, err
	}
	return sb, nil
}

func (s *Sandboxes) Get(sandbox string) (*Sandbox, error) {
	sb := &Sandbox{c: s.c}
	if err := s.c.call("GET", "/v1/sandboxes/"+seg(sandbox), nil, nil, &sb.SandboxDoc); err != nil {
		return nil, err
	}
	return sb, nil
}

// List answers the caller's sandboxes, newest first; team scopes to theirs in
// that team.
func (s *Sandboxes) List(team string) ([]*Sandbox, error) {
	q := url.Values{}
	if team != "" {
		q.Set("team", team)
	}
	var docs []SandboxDoc
	if err := s.c.call("GET", "/v1/sandboxes", q, nil, &docs); err != nil {
		return nil, err
	}
	out := make([]*Sandbox, len(docs))
	for i, d := range docs {
		out[i] = &Sandbox{SandboxDoc: d, c: s.c}
	}
	return out, nil
}

// Run is create, exec, wait, read and terminate in one call. The sandbox is
// ALWAYS terminated, whatever failed; a terminate failure after a clean run is
// returned beside the result.
func (s *Sandboxes) Run(req RunSandbox) (res *RunResult, err error) {
	// A command allowed longer than the sandbox's default lifetime must not be
	// cut by it: the sandbox outlives the wait by a minute, within 10 s..24 h.
	if req.Timeout > 0 && req.Sandbox.Timeout == 0 {
		req.Sandbox.Timeout = min(24*time.Hour, max(10*time.Second, req.Timeout+time.Minute))
	}
	sb, err := s.Create(req.Sandbox)
	if err != nil {
		return nil, err
	}
	defer func() {
		if terr := sb.Terminate(); terr != nil && err == nil {
			err = terr
		}
	}()
	p, err := sb.Exec(ExecCommand{Cmd: req.Cmd, Cwd: req.Cwd, Env: req.Env})
	if err != nil {
		return nil, err
	}
	code, err := p.Wait(req.Timeout)
	if err != nil {
		return nil, err
	}
	stdout, stderr, err := p.collect()
	if err != nil {
		return nil, err
	}
	return &RunResult{ExitCode: code, Stdout: stdout, Stderr: stderr, Sandbox: sb.ID}, nil
}

func (sb *Sandbox) path(tail string) string { return "/v1/sandboxes/" + seg(sb.ID) + tail }

// Refresh re-reads the doc.
func (sb *Sandbox) Refresh() error {
	var d SandboxDoc
	if err := sb.c.call("GET", sb.path(""), nil, nil, &d); err != nil {
		return err
	}
	sb.SandboxDoc = d
	return nil
}

// terminated is the error a sandbox that ended answers: an *ApiError (409, as
// the server answers a tool call on it) naming the reason.
func (sb *Sandbox) terminated() error {
	msg := fmt.Sprintf("sandbox %s is terminated (%s)", sb.ID, sb.Reason)
	if sb.Message != "" {
		msg += ": " + sb.Message
	}
	return &ApiError{Status: 409, Message: msg}
}

// poll re-reads the doc until done says stop, backing off from 0.5 s to 2 s;
// a zero timeout means no limit.
func (sb *Sandbox) poll(timeout time.Duration, done func() bool) error {
	var deadline time.Time
	if timeout > 0 {
		deadline = time.Now().Add(timeout)
	}
	delay := 500 * time.Millisecond
	for {
		if err := sb.Refresh(); err != nil {
			return err
		}
		if done() {
			return nil
		}
		if !deadline.IsZero() {
			left := time.Until(deadline)
			if left <= 0 {
				return fmt.Errorf("sandbox %s is %s after %s: %w", sb.ID, sb.Phase, timeout, ErrTimeout)
			}
			delay = min(delay, left)
		}
		time.Sleep(delay)
		delay = min(delay*2, 2*time.Second)
	}
}

// WaitRunning polls until the sandbox runs (a zero timeout means 120 s). One
// that terminates instead is an *ApiError naming the reason.
func (sb *Sandbox) WaitRunning(timeout time.Duration) error {
	if timeout == 0 {
		timeout = 120 * time.Second
	}
	if err := sb.poll(timeout, func() bool { return sb.Phase == "running" || sb.Phase == "terminated" }); err != nil {
		return err
	}
	if sb.Phase == "terminated" {
		return sb.terminated()
	}
	return nil
}

// Wait polls until the sandbox has terminated and answers the reason; a zero
// timeout means no limit.
func (sb *Sandbox) Wait(timeout time.Duration) (string, error) {
	if sb.Phase == "terminated" {
		return sb.Reason, nil
	}
	if err := sb.poll(timeout, func() bool { return sb.Phase == "terminated" }); err != nil {
		return "", err
	}
	return sb.Reason, nil
}

// Exec starts a process (waiting for the sandbox to run first unless it is
// already known to) and answers it at once.
func (sb *Sandbox) Exec(req ExecCommand) (*Process, error) {
	if sb.Phase != "running" {
		if err := sb.WaitRunning(0); err != nil {
			return nil, err
		}
	}
	a := toolArgs{"cmd": req.Cmd, "detach": true}.str("cwd", req.Cwd).val("env", req.Env, req.Env != nil)
	var out struct {
		ID json.RawMessage `json:"id"`
	}
	if err := sb.tool("exec", a, &out); err != nil {
		return nil, err
	}
	id := strings.Trim(string(out.ID), `"`)
	if id == "" || id == "null" {
		return nil, fmt.Errorf("the exec answer named no process id")
	}
	p := &Process{ID: id, Sandbox: sb}
	p.Stdin = &ProcessStdin{p: p}
	p.Stdout = &ProcessOutput{p: p, stream: "stdout"}
	p.Stderr = &ProcessOutput{p: p, stream: "stderr"}
	return p, nil
}

// Read answers the read tool's JSON for path (relative to ~/workspace).
func (sb *Sandbox) Read(path string) (any, error) {
	var out any
	err := sb.tool("read", map[string]any{"path": path}, &out)
	return out, err
}

// Write answers the write tool's JSON; Path is relative to ~/workspace.
func (sb *Sandbox) Write(req WriteFile) (any, error) {
	var out any
	err := sb.tool("write", map[string]any{"path": req.Path, "content": req.Content}, &out)
	return out, err
}

// Terminate asks the sandbox to end (202) and keeps the doc it answers.
func (sb *Sandbox) Terminate() error {
	var d SandboxDoc
	if err := sb.c.call("POST", sb.path("/terminate"), nil, nil, &d); err != nil {
		return err
	}
	if d.ID != "" {
		sb.SandboxDoc = d
	}
	return nil
}

// Delete deletes the sandbox's record (and with it the sandbox).
func (sb *Sandbox) Delete() error { return sb.c.call("DELETE", sb.path(""), nil, nil, nil) }

func (sb *Sandbox) tool(name string, args map[string]any, out any) error {
	return sb.c.call("POST", sb.path("/tools/"+seg(name)), nil, args, out)
}

// Process is one detached process in a sandbox.
type Process struct {
	ID      string
	Sandbox *Sandbox
	Stdin   *ProcessStdin
	Stdout  *ProcessOutput
	Stderr  *ProcessOutput

	mu     sync.Mutex
	exited bool
	code   int
}

// waitAnswer is process_wait's (and process_kill's) answer.
type waitAnswer struct {
	State    string `json:"state"`
	ExitCode *int   `json:"exit_code"`
}

// record keeps an exited answer; -1 stands for an exit with no code.
func (p *Process) record(w waitAnswer) (int, bool) {
	if w.State != "exited" {
		return -1, false
	}
	code := -1
	if w.ExitCode != nil {
		code = *w.ExitCode
	}
	p.mu.Lock()
	p.exited, p.code = true, code
	p.mu.Unlock()
	return code, true
}

// slice is one process_wait's length: the server's 20 s, kept under this
// client's own timeout so a quiet wait never reads as a dead connection.
func (p *Process) slice() time.Duration {
	s := maxWaitSlice
	if t := p.Sandbox.c.http.Timeout; t > 0 && t-5*time.Second < s {
		s = max(t-5*time.Second, time.Second)
	}
	return s
}

// Wait blocks until the process exits and answers its exit code (-1 when it
// ended without one, e.g. by a signal). A zero timeout means no limit; past
// it the error wraps ErrTimeout and the process keeps running.
func (p *Process) Wait(timeout time.Duration) (int, error) {
	var deadline time.Time
	if timeout > 0 {
		deadline = time.Now().Add(timeout)
	}
	for {
		if code, ok := p.lastExit(); ok {
			return code, nil
		}
		ms := p.slice()
		if !deadline.IsZero() {
			left := time.Until(deadline)
			if left <= 0 {
				return -1, fmt.Errorf("process %s still running after %s: %w", p.ID, timeout, ErrTimeout)
			}
			ms = min(ms, left)
		}
		var w waitAnswer
		if err := p.Sandbox.tool("process_wait", map[string]any{"id": p.ID, "timeout_ms": ms.Milliseconds()}, &w); err != nil {
			return -1, err
		}
		if code, ok := p.record(w); ok {
			return code, nil
		}
	}
}

// Poll answers the exit code if the process has exited, without waiting.
func (p *Process) Poll() (code int, exited bool, err error) {
	if code, ok := p.lastExit(); ok {
		return code, true, nil
	}
	var w waitAnswer
	if err := p.Sandbox.tool("process_wait", map[string]any{"id": p.ID, "timeout_ms": 0}, &w); err != nil {
		return -1, false, err
	}
	code, exited = p.record(w)
	return code, exited, nil
}

func (p *Process) lastExit() (int, bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.code, p.exited
}

// ExitCode is the last known exit code: -1 while the process has not been
// seen to exit, or when it ended without one — as os.ProcessState.ExitCode.
func (p *Process) ExitCode() int {
	code, ok := p.lastExit()
	if !ok {
		return -1
	}
	return code
}

// Kill signals the process group; an empty signal means TERM.
func (p *Process) Kill(signal string) error {
	return p.Sandbox.tool("process_kill", toolArgs{"id": p.ID}.str("signal", signal), nil)
}

// collect follows ONE stream for both outputs — what Run reads.
func (p *Process) collect() (string, string, error) {
	var out, errb strings.Builder
	err := p.Sandbox.c.stream(p.streamPath(), func(f frame) bool {
		switch f.Stream {
		case "stdout":
			out.WriteString(f.Data)
		case "stderr":
			errb.WriteString(f.Data)
		}
		return true
	})
	return out.String(), errb.String(), err
}

func (p *Process) streamPath() string {
	return p.Sandbox.path("/processes/" + seg(p.ID) + "/stream")
}

// ProcessStdin is a process's stdin: an io.Writer (and io.StringWriter) whose
// every Write is delivered by the time it returns.
type ProcessStdin struct{ p *Process }

// Write sends b, base64-encoded, so any bytes survive the JSON trip.
func (s *ProcessStdin) Write(b []byte) (int, error) {
	if len(b) == 0 {
		return 0, nil
	}
	args := map[string]any{"id": s.p.ID, "data_base64": base64.StdEncoding.EncodeToString(b)}
	if err := s.p.Sandbox.tool("process_write", args, nil); err != nil {
		return 0, err
	}
	return len(b), nil
}

func (s *ProcessStdin) WriteString(str string) (int, error) { return s.Write([]byte(str)) }

// WriteEOF closes stdin: the process reads end of file, and later writes fail.
func (s *ProcessStdin) WriteEOF() error {
	return s.p.Sandbox.tool("process_close_stdin", map[string]any{"id": s.p.ID}, nil)
}

// Drain is a no-op kept for Modal's shape: a Write has already landed when it
// returns.
func (s *ProcessStdin) Drain() error { return nil }

// ProcessOutput is one of a process's two output streams. Each call follows
// the process's stream from its first byte to its exit; the server's ring
// keeps the last 4 MiB, so older output of a chatty process is gone.
type ProcessOutput struct {
	p      *Process
	stream string
}

// ReadAll answers the whole stream once the process has exited.
func (o *ProcessOutput) ReadAll() (string, error) {
	var b strings.Builder
	err := o.chunks(func(data string) bool {
		b.WriteString(data)
		return true
	})
	return b.String(), err
}

// Lines calls fn with each line as it arrives, without its "\n"; the trailing
// partial line, if any, comes last. fn returning false stops early.
func (o *ProcessOutput) Lines(fn func(line string) bool) error {
	var partial strings.Builder
	stopped := false
	err := o.chunks(func(data string) bool {
		for {
			i := strings.IndexByte(data, '\n')
			if i < 0 {
				partial.WriteString(data)
				return true
			}
			partial.WriteString(data[:i])
			data = data[i+1:]
			line := partial.String()
			partial.Reset()
			if !fn(line) {
				stopped = true
				return false
			}
		}
	})
	if err == nil && !stopped && partial.Len() > 0 {
		fn(partial.String())
	}
	return err
}

func (o *ProcessOutput) chunks(fn func(data string) bool) error {
	return o.p.Sandbox.c.stream(o.p.streamPath(), func(f frame) bool {
		if f.Stream != o.stream {
			return true
		}
		return fn(f.Data)
	})
}

// frame is one NDJSON line of a process stream; Exit marks the last one.
type frame struct {
	Stream string `json:"stream"`
	Data   string `json:"data"`
	Exit   bool   `json:"-"`
}

// stream opens a GET that answers NDJSON and hands each frame to fn until the
// exit frame, the end of the body, or fn returning false. It runs on its own
// client with no overall timeout: a stream lasts as long as the process.
func (c *Client) stream(path string, fn func(frame) bool) error {
	req, err := http.NewRequest("GET", c.baseURL+path, nil)
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", "kloudlite-sdk-go/"+Version)
	req.Header.Set("Accept", "application/x-ndjson")
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}
	resp, err := (&http.Client{Transport: c.http.Transport}).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		raw, _ := io.ReadAll(resp.Body)
		return &ApiError{Status: resp.StatusCode, Message: errorText(raw)}
	}
	// ReadBytes rather than a Scanner: one frame can carry a whole 4 MiB ring.
	r := bufio.NewReader(resp.Body)
	for {
		line, rerr := r.ReadBytes('\n')
		if len(strings.TrimSpace(string(line))) > 0 {
			var raw map[string]json.RawMessage
			if err := json.Unmarshal(line, &raw); err != nil {
				return fmt.Errorf("a stream frame is not JSON: %w", err)
			}
			var f frame
			_ = json.Unmarshal(raw["stream"], &f.Stream)
			_ = json.Unmarshal(raw["data"], &f.Data)
			_, f.Exit = raw["exit"]
			if !fn(f) || f.Exit {
				return nil
			}
		}
		if rerr == io.EOF {
			return nil
		}
		if rerr != nil {
			return rerr
		}
	}
}
