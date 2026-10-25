package kloudlite

// Sandboxes: throwaway, isolated machines that run ONE command, over
// /v1/sandboxes. Create returns at once; the sandbox ends when the command
// exits or after Timeout:
//
//	sb, err := kl.Sandboxes.Create(kloudlite.CreateSandbox{Cmd: []string{"python3", "-c", "print(6 * 7)"}, Packages: []string{"python3"}})
//	code, err := sb.Wait(0)
//	out, err := sb.Logs(false)
//
// There is no exec, read or write on a sandbox: the command is its whole job.
// Attach (the interactive shell) is a WebSocket and is not in this SDK.

import (
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"time"
)

// ErrTimeout is wrapped by every wait that ran out of time (errors.Is).
var ErrTimeout = errors.New("timed out")

// maxWaitSlice is the server's ceiling on one /wait.
const maxWaitSlice = 20 * time.Second

// Sandboxes is the sandbox module: /v1/sandboxes.
type Sandboxes struct{ c *Client }

// CreateSandbox carries what a create takes; every zero field is left out and
// means the server's default (the platform image, 300 s, 1 core, 2 GB, 10 GB,
// network "internet"). Cmd is required: the program, then its arguments. Tty
// runs it on a pty; Stdin keeps input open for Attach. FromWorkspace (an id,
// "name" or "team/name") or FromSnapshot (a volume name, name@N or a snapshot
// id) seed the disk. Timeout (10 s..24 h) is the sandbox's whole lifetime and
// travels in whole seconds. A session's sandbox is made with
// Sessions.CreateSandbox.
type CreateSandbox struct {
	Cmd           []string
	Image         string
	Packages      []string
	FromWorkspace string
	FromSnapshot  string
	Team          string
	Tty           bool
	Stdin         bool
	Timeout       time.Duration
	// Network is "internet" or "none".
	Network  string
	Env      map[string]string
	Cpu      int
	MemoryGb int
	DiskGb   int
}

// RunSandbox is Run's input: Sandbox (with its Cmd) is created, waited for and
// terminated. Timeout bounds the wait (0 = until it ends); the sandbox is
// terminated either way.
type RunSandbox struct {
	Sandbox CreateSandbox
	Timeout time.Duration
}

// RunResult is what Run answers: the exit code (-1 when the sandbox ended
// without one), the output, and the sandbox's id. The sandbox keeps ONE
// stream, so Stdout holds the output and error output together and Stderr is
// always "".
type RunResult struct {
	ExitCode int
	Stdout   string
	Stderr   string
	Sandbox  string
}

// ListSandboxes pages the caller's sandboxes: Limit is 1..=200 (server default
// 50), Cursor the next-cursor of the page before.
type ListSandboxes struct {
	Team   string
	Limit  int
	Cursor string
}

// SandboxDoc is the server's view of a sandbox. Phase is pending, starting,
// running or terminated; Reason (once terminated) is Terminated, Timeout,
// Idle, Exited, Failed, Unplaceable, SourceGone, Lost or EnvironmentGone.
// ExitCode is set when the command exited. Times are RFC 3339.
type SandboxDoc struct {
	ID          string            `json:"id"`
	Owner       string            `json:"owner"`
	Team        string            `json:"team"`
	Region      string            `json:"region"`
	Image       string            `json:"image"`
	Packages    []string          `json:"packages"`
	Phase       string            `json:"phase"`
	Reason      string            `json:"reason"`
	Message     string            `json:"message"`
	ExitCode    *int              `json:"exit_code"`
	CreatedAt   string            `json:"created_at"`
	StartedAt   string            `json:"started_at"`
	FinishedAt  string            `json:"finished_at"`
	TimeoutSecs int64             `json:"timeout_secs"`
	Network     string            `json:"network"`
	Cpu         int64             `json:"cpu"`
	MemoryGb    int64             `json:"memory_gb"`
	DiskGb      int64             `json:"disk_gb"`
	From        map[string]string `json:"from"`
}

// Sandbox is a handle on one sandbox; its doc is the last one read (Refresh
// re-reads it). A handle is not safe for concurrent use.
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

// body is the POST /v1/sandboxes body; it refuses a missing Cmd before anything is sent.
func (r CreateSandbox) body() (map[string]any, error) {
	if len(r.Cmd) == 0 {
		return nil, errors.New("cmd is required: the program and its arguments as a non-empty list of strings")
	}
	a := toolArgs{"cmd": r.Cmd}.str("image", r.Image).val("packages", r.Packages, r.Packages != nil).
		str("from_workspace", r.FromWorkspace).str("from_snapshot", r.FromSnapshot).str("team", r.Team).
		flag("tty", r.Tty).flag("stdin", r.Stdin).num("timeout_secs", secs(r.Timeout)).
		str("network", r.Network).val("env", r.Env, r.Env != nil).
		num("cpu", int64(r.Cpu)).num("memory_gb", int64(r.MemoryGb)).num("disk_gb", int64(r.DiskGb))
	return a, nil
}

// Create asks for a sandbox and returns at once (202) — it does not wait for
// it to run; it is usually still pending.
func (s *Sandboxes) Create(req CreateSandbox) (*Sandbox, error) {
	body, err := req.body()
	if err != nil {
		return nil, err
	}
	sb := &Sandbox{c: s.c}
	if err := s.c.call("POST", "/v1/sandboxes", nil, body, &sb.SandboxDoc); err != nil {
		return nil, err
	}
	return sb, nil
}

// Get answers one sandbox by id (a name does not resolve).
func (s *Sandboxes) Get(sandbox string) (*Sandbox, error) {
	sb := &Sandbox{c: s.c}
	if err := s.c.call("GET", "/v1/sandboxes/"+seg(sandbox), nil, nil, &sb.SandboxDoc); err != nil {
		return nil, err
	}
	return sb, nil
}

// List answers ONE page of the caller's sandboxes, newest first; team scopes to
// that team. ListPage takes a limit and cursor and answers the next cursor.
func (s *Sandboxes) List(team string) ([]*Sandbox, error) {
	out, _, err := s.ListPage(ListSandboxes{Team: team})
	return out, err
}

// ListPage answers a page and the cursor of the next one ("" on the last page).
func (s *Sandboxes) ListPage(req ListSandboxes) ([]*Sandbox, string, error) {
	q := url.Values{}
	if req.Team != "" {
		q.Set("team", req.Team)
	}
	if req.Limit > 0 {
		q.Set("limit", strconv.Itoa(req.Limit))
	}
	if req.Cursor != "" {
		q.Set("cursor", req.Cursor)
	}
	raw, h, err := s.c.do(s.c.http, "GET", "/v1/sandboxes", q, nil, "application/json")
	if err != nil {
		return nil, "", err
	}
	var docs []SandboxDoc
	if len(raw) > 0 {
		if err := unmarshal(raw, &docs); err != nil {
			return nil, "", err
		}
	}
	out := make([]*Sandbox, len(docs))
	for i, d := range docs {
		out[i] = &Sandbox{SandboxDoc: d, c: s.c}
	}
	return out, h.Get("X-Next-Cursor"), nil
}

// Run is create, wait, read the output and terminate in one call. The sandbox
// is ALWAYS terminated, whatever failed; a terminate failure after a clean run
// is returned beside the result.
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
	code, err := sb.Wait(req.Timeout)
	if err != nil {
		return nil, err
	}
	out, err := sb.Logs(false)
	if err != nil {
		return nil, err
	}
	return &RunResult{ExitCode: code, Stdout: out, Stderr: "", Sandbox: sb.ID}, nil
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
// the server answers a call on it) naming the reason.
func (sb *Sandbox) terminated() error {
	msg := fmt.Sprintf("sandbox %s is terminated (%s)", sb.ID, sb.Reason)
	if sb.Message != "" {
		msg += ": " + sb.Message
	}
	return &ApiError{Status: 409, Message: msg}
}

// WaitRunning polls until the sandbox runs (a zero timeout means 120 s),
// backing off from 0.5 s to 2 s. One that terminates instead (an image that
// does not pull, a quota, a command that already exited) is an *ApiError
// naming the reason.
func (sb *Sandbox) WaitRunning(timeout time.Duration) error {
	if timeout == 0 {
		timeout = 120 * time.Second
	}
	deadline := time.Now().Add(timeout)
	delay := 500 * time.Millisecond
	for {
		if err := sb.Refresh(); err != nil {
			return err
		}
		switch sb.Phase {
		case "running":
			return nil
		case "terminated":
			return sb.terminated()
		}
		left := time.Until(deadline)
		if left <= 0 {
			return fmt.Errorf("sandbox %s not running (%s) after %s: %w", sb.ID, sb.Phase, timeout, ErrTimeout)
		}
		time.Sleep(min(delay, left))
		delay = min(delay*3/2, 2*time.Second)
	}
}

// Logs answers the main process's output (stdout and stderr together) as text:
// the live output while it runs, the last 256 KiB after. With follow it holds
// until the process ends. 409 while the sandbox has no output yet.
func (sb *Sandbox) Logs(follow bool) (string, error) {
	q := url.Values{}
	hc := sb.c.http
	if follow {
		q.Set("follow", "true")
		hc = &http.Client{Transport: hc.Transport} // no overall timeout: it lasts as long as the process
	}
	raw, _, err := sb.c.do(hc, "GET", sb.path("/logs"), q, nil, "text/plain")
	return string(raw), err
}

// Shell is the attach handle; it is never made: Attach answers ErrNotImplemented.
type Shell struct{}

func (*Shell) Read([]byte) (int, error)  { return 0, ErrNotImplemented }
func (*Shell) Write([]byte) (int, error) { return 0, ErrNotImplemented }
func (*Shell) Close() error              { return ErrNotImplemented }
func (*Shell) Wait() (int, error)        { return -1, ErrNotImplemented }

// Attach is not in this SDK: GET /v1/sandboxes/{id}/attach is a WebSocket and
// this client uses only the standard library.
func (sb *Sandbox) Attach(cols, rows int) (*Shell, error) {
	return nil, fmt.Errorf("sandbox attach is a WebSocket and is not in the Go SDK: %w", ErrNotImplemented)
}

// Terminate stops the sandbox for good (202, or 200 if already ended) and keeps
// the doc it answers; the record stays until Delete.
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

// Wait blocks until the sandbox has ended and answers its exit code (-1 when it
// ended some other way than the command exiting; Reason says how). It waits in
// steps of at most 20 s, the most the api holds one open. A zero timeout means
// no limit; past it the error wraps ErrTimeout.
func (sb *Sandbox) Wait(timeout time.Duration) (int, error) {
	var deadline time.Time
	if timeout > 0 {
		deadline = time.Now().Add(timeout)
	}
	step := maxWaitSlice
	if t := sb.c.http.Timeout; t > 0 && t-5*time.Second < step { // a quiet wait must not read as a dead connection
		step = max(t-5*time.Second, time.Second)
	}
	for {
		ms := step
		if !deadline.IsZero() {
			left := time.Until(deadline)
			if left <= 0 {
				return -1, fmt.Errorf("sandbox %s still %s after %s: %w", sb.ID, sb.Phase, timeout, ErrTimeout)
			}
			ms = min(ms, left)
		}
		var out struct {
			Phase    *string `json:"phase"`
			Reason   *string `json:"reason"`
			Message  *string `json:"message"`
			ExitCode *int    `json:"exit_code"`
			hasCode  bool
		}
		var raw map[string]any
		if err := sb.c.call("POST", sb.path("/wait"), nil, map[string]int64{"timeout_ms": ms.Milliseconds()}, &raw); err != nil {
			return -1, err
		}
		_, out.hasCode = raw["exit_code"]
		if err := remarshal(raw, &out); err != nil {
			return -1, err
		}
		if out.Phase != nil {
			sb.Phase = *out.Phase
		}
		if out.Reason != nil {
			sb.Reason = *out.Reason
		}
		if out.Message != nil {
			sb.Message = *out.Message
		}
		if out.hasCode {
			sb.ExitCode = out.ExitCode
		}
		if sb.Phase == "terminated" {
			if sb.ExitCode == nil {
				return -1, nil
			}
			return *sb.ExitCode, nil
		}
	}
}

// Delete deletes the sandbox's record (204); one still running dies with it.
func (sb *Sandbox) Delete() error { return sb.c.call("DELETE", sb.path(""), nil, nil, nil) }
