package kloudlite

// Pipelines: CI over /v1/pipelines. Definitions live as YAML in an owner's
// "{owner}/pipelines" repository and run against that owner's repositories,
// each job in its own sandbox. A run is recorded on the repository it is
// about, so it is read back by that Repo ("owner/name"):
//
//	run, err := kl.Pipelines.Run(kloudlite.RunPipeline{Owner: "acme", Pipeline: "ci", Repo: "acme/payments"})
//	err = run.Follow(kloudlite.FollowLogs{Job: "test"}, func(chunk string) bool {
//		fmt.Print(chunk)
//		return true
//	})
//	state, err := run.Wait(kloudlite.WaitRun{})
//
// A log is paged from a byte offset and followed by polling: the worker writes
// a running job's log every 15 s, so there is nothing to stream.

import (
	"fmt"
	"net/url"
	"slices"
	"time"
)

// Pipelines is the pipelines module: /v1/pipelines.
type Pipelines struct{ c *Client }

// PipelineCatalog is what List answers: the definitions on {owner}/pipelines's
// main branch at Sha, broken ones included. Errors are the shared actions'
// own, which belong to no one pipeline.
type PipelineCatalog struct {
	Repo      string        `json:"repo"`
	Sha       string        `json:"sha"`
	Pipelines []PipelineDef `json:"pipelines"`
	Errors    []string      `json:"errors"`
}

// PipelineDef is one definition; a broken one has Valid false and its Errors.
// On is its triggers as the definition spells them (nil when it has none),
// Jobs its job names in the order written.
type PipelineDef struct {
	Name   string         `json:"name"`
	File   string         `json:"file"`
	Valid  bool           `json:"valid"`
	Errors []string       `json:"errors"`
	Repos  []string       `json:"repos"`
	On     map[string]any `json:"on"`
	Jobs   []string       `json:"jobs"`
}

// RunPipeline starts Owner's Pipeline by hand on Repo (a name, or
// "owner/name" — one of Owner's) at Ref (a branch, tag, full ref or sha;
// empty means main). Inputs are the ones the pipeline's on.manual.inputs
// declares. Environment runs every job that names none in that environment;
// Clone instead in a per-run copy of it, deleted when the run ends — one or
// the other.
type RunPipeline struct {
	Owner       string
	Pipeline    string
	Repo        string
	Ref         string
	Inputs      map[string]string
	Environment string
	Clone       string
}

// ListRuns reads Repo's runs, newest first; Pipeline keeps one pipeline's, a
// zero Limit is the server's default.
type ListRuns struct {
	Repo     string
	Pipeline string
	Limit    int
}

// GetRun reads one run of Repo by its id ("1760400000000-0a1b2c").
type GetRun struct {
	Repo string
	Run  string
}

// WaitRun bounds Run.Wait: a zero Timeout means no limit, a zero Poll 5 s.
type WaitRun struct {
	Timeout time.Duration
	Poll    time.Duration
}

// FollowLogs names the job Run.Follow follows — Job is its index in Jobs (an
// int) or its Key (a string) — and how often it polls while nothing new has
// landed (zero means 5 s).
type FollowLogs struct {
	Job  any
	Poll time.Duration
}

// RunDoc is a run as /v1 answers it. State is queued, running, success,
// failure or cancelled; Event is manual, push, pull_request, schedule or
// pipeline. Environment is as requested: a string, {"clone": name}, or nil
// for the pipeline's own. Times are unix milliseconds, zero while unset.
type RunDoc struct {
	ID              string         `json:"id"`
	Number          int64          `json:"number"`
	Pipeline        string         `json:"pipeline"`
	PipelinesRepo   string         `json:"pipelinesRepo"`
	PipelinesSha    string         `json:"pipelinesSha"`
	Repo            string         `json:"repo"`
	Ref             string         `json:"ref"`
	Sha             string         `json:"sha"`
	Event           string         `json:"event"`
	Inputs          map[string]any `json:"inputs"`
	Environment     any            `json:"environment"`
	RequestedBy     string         `json:"requestedBy"`
	RequestedAtMs   int64          `json:"requestedAtMs"`
	State           string         `json:"state"`
	CancelRequested bool           `json:"cancelRequested"`
	StartedAtMs     int64          `json:"startedAtMs"`
	FinishedAtMs    int64          `json:"finishedAtMs"`
	Detail          string         `json:"detail"`
	Jobs            []RunJob       `json:"jobs"`
}

// RunJob is one job of a run, matrix instances expanded: Key is unique in the
// run ("test (postgres16)"), Job the name in the definition. State is
// pending, running, success, failure, skipped or cancelled; ExitCode is nil
// until it has one.
type RunJob struct {
	Key          string         `json:"key"`
	Job          string         `json:"job"`
	Matrix       map[string]any `json:"matrix"`
	Needs        []string       `json:"needs"`
	State        string         `json:"state"`
	Sandbox      string         `json:"sandbox"`
	StartedAtMs  int64          `json:"startedAtMs"`
	FinishedAtMs int64          `json:"finishedAtMs"`
	ExitCode     *int           `json:"exitCode"`
	FailedStep   string         `json:"failedStep"`
	Detail       string         `json:"detail"`
}

// LogPage is one page of a job's log from a byte offset; poll with Next until
// Complete — the job has ended and every byte was returned.
type LogPage struct {
	Data     string `json:"data"`
	Next     int64  `json:"next"`
	Complete bool   `json:"complete"`
	State    string `json:"state"`
}

// Run is a handle on one pipeline run; its doc is the last one read (Refresh
// re-reads it). A handle is not safe for concurrent Refresh calls.
type Run struct {
	RunDoc
	c *Client
}

func runsPath(repo, tail string) (string, error) {
	return repoPath(repo, "/runs"+tail)
}

// List answers owner's pipeline definitions; an empty owner means the
// caller's own.
func (p *Pipelines) List(owner string) (*PipelineCatalog, error) {
	q := url.Values{}
	if owner != "" {
		q.Set("owner", owner)
	}
	var out PipelineCatalog
	if err := p.c.call("GET", "/v1/pipelines", q, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Run starts a run and answers it at once (202), queued. A definition that
// cannot run is an *ApiError 422 whose Message lists every reason, and
// nothing is recorded.
func (p *Pipelines) Run(req RunPipeline) (*Run, error) {
	if req.Environment != "" && req.Clone != "" {
		return nil, fmt.Errorf("set Environment or Clone, not both")
	}
	body := toolArgs{"repo": req.Repo}.str("ref", req.Ref).val("inputs", req.Inputs, req.Inputs != nil).
		str("environment", req.Environment).val("environment", map[string]string{"clone": req.Clone}, req.Clone != "")
	r := &Run{c: p.c}
	path := "/v1/pipelines/" + seg(req.Owner) + "/" + seg(req.Pipeline) + "/runs"
	if err := p.c.call("POST", path, nil, map[string]any(body), &r.RunDoc); err != nil {
		return nil, err
	}
	return r, nil
}

// Runs answers Repo's runs, newest first.
func (p *Pipelines) Runs(req ListRuns) ([]*Run, error) {
	path, err := runsPath(req.Repo, "")
	if err != nil {
		return nil, err
	}
	q := url.Values{}
	if req.Pipeline != "" {
		q.Set("pipeline", req.Pipeline)
	}
	if req.Limit > 0 {
		q.Set("limit", fmt.Sprint(req.Limit))
	}
	var docs []RunDoc
	if err := p.c.call("GET", path, q, nil, &docs); err != nil {
		return nil, err
	}
	out := make([]*Run, len(docs))
	for i, d := range docs {
		out[i] = &Run{RunDoc: d, c: p.c}
	}
	return out, nil
}

// GetRun answers one run.
func (p *Pipelines) GetRun(req GetRun) (*Run, error) {
	path, err := runsPath(req.Repo, "/"+seg(req.Run))
	if err != nil {
		return nil, err
	}
	r := &Run{c: p.c}
	if err := p.c.call("GET", path, nil, nil, &r.RunDoc); err != nil {
		return nil, err
	}
	return r, nil
}

func (r *Run) path(tail string) (string, error) {
	return runsPath(r.Repo, "/"+seg(r.ID)+tail)
}

// Done reports whether the run has ended (success, failure or cancelled) as
// of the last read.
func (r *Run) Done() bool {
	return r.State == "success" || r.State == "failure" || r.State == "cancelled"
}

// Refresh re-reads the doc.
func (r *Run) Refresh() error {
	path, err := r.path("")
	if err != nil {
		return err
	}
	var d RunDoc
	if err := r.c.call("GET", path, nil, nil, &d); err != nil {
		return err
	}
	r.RunDoc = d
	return nil
}

// Cancel asks the run to stop: a queued run is cancelled at once, a running
// one once its worker has ended its sandboxes — Wait sees it reach cancelled.
func (r *Run) Cancel() error {
	path, err := r.path("/cancel")
	if err != nil {
		return err
	}
	var d RunDoc
	if err := r.c.call("POST", path, nil, nil, &d); err != nil {
		return err
	}
	if d.ID != "" {
		r.RunDoc = d
	}
	return nil
}

// Wait polls until the run has ended and answers its final state. Past
// req.Timeout the error wraps ErrTimeout and the run carries on.
func (r *Run) Wait(req WaitRun) (string, error) {
	poll := req.Poll
	if poll <= 0 {
		poll = 5 * time.Second
	}
	var deadline time.Time
	if req.Timeout > 0 {
		deadline = time.Now().Add(req.Timeout)
	}
	for !r.Done() {
		if err := r.Refresh(); err != nil {
			return "", err
		}
		if r.Done() {
			break
		}
		wait := poll
		if !deadline.IsZero() {
			left := time.Until(deadline)
			if left <= 0 {
				return r.State, fmt.Errorf("run %s still %s after %s: %w", r.ID, r.State, req.Timeout, ErrTimeout)
			}
			wait = min(wait, left)
		}
		time.Sleep(wait)
	}
	return r.State, nil
}

// Logs answers the job's whole log so far (combined stdout and stderr), every
// page of it. job is its index in Jobs (an int) or its Key (a string).
func (r *Run) Logs(job any) (string, error) {
	index, err := r.jobIndex(job)
	if err != nil {
		return "", err
	}
	var out []byte
	for offset := int64(0); ; {
		page, err := r.logPage(index, offset)
		if err != nil {
			return "", err
		}
		out = append(out, page.Data...)
		if page.Complete || page.Next <= offset {
			return string(out), nil
		}
		offset = page.Next
	}
}

// Follow calls fn with the job's log in chunks as it is written — from the
// first byte, polling while nothing new has landed — until the job has ended
// and every byte was delivered, or fn returns false. The callback is the one
// input beside the request struct: it is not a value whose order could be
// confused with it.
func (r *Run) Follow(req FollowLogs, fn func(chunk string) bool) error {
	index, err := r.jobIndex(req.Job)
	if err != nil {
		return err
	}
	poll := req.Poll
	if poll <= 0 {
		poll = 5 * time.Second
	}
	for offset := int64(0); ; {
		page, err := r.logPage(index, offset)
		if err != nil {
			return err
		}
		if page.Data != "" && !fn(page.Data) {
			return nil
		}
		if page.Complete {
			return nil
		}
		if page.Next <= offset {
			time.Sleep(poll)
		}
		offset = max(offset, page.Next)
	}
}

func (r *Run) logPage(index int, offset int64) (*LogPage, error) {
	path, err := r.path(fmt.Sprintf("/jobs/%d/logs", index))
	if err != nil {
		return nil, err
	}
	var page LogPage
	if err := r.c.call("GET", path, url.Values{"offset": {fmt.Sprint(offset)}}, nil, &page); err != nil {
		return nil, err
	}
	return &page, nil
}

func (r *Run) jobIndex(job any) (int, error) {
	keys := make([]string, len(r.Jobs))
	for i, j := range r.Jobs {
		keys[i] = j.Key
	}
	switch j := job.(type) {
	case int:
		if j < 0 || j >= len(keys) {
			return 0, fmt.Errorf("run %s has %d jobs, no job %d", r.ID, len(keys), j)
		}
		return j, nil
	case string:
		if i := slices.Index(keys, j); i >= 0 {
			return i, nil
		}
		return 0, fmt.Errorf("run %s has no job %q; its jobs are %q", r.ID, j, keys)
	default:
		return 0, fmt.Errorf("a job is its index (int) or its key (string), got %T", job)
	}
}
