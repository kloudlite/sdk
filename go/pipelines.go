package kloudlite

// Pipelines: /v1/pipelines. A pipeline is a JSON definition stored as numbered versions;
// a run executes one version, each job in its own sandbox:
//
//	run, err := kl.Pipelines.Run(kloudlite.RunPipeline{Owner: "acme", Pipeline: "test", Inputs: map[string]string{"target": "staging"}})
//	err = run.Follow(kloudlite.FollowLogs{Job: "test"}, func(chunk string) bool {
//		fmt.Print(chunk)
//		return true
//	})
//	state, err := run.Wait(kloudlite.WaitRun{})
//
// A log is paged from a byte offset and followed by polling: the worker writes
// a running job's log every 15 s, so there is nothing to stream.

import (
	"encoding/json"
	"fmt"
	"net/url"
	"slices"
	"time"
)

// Pipelines is the pipelines module: /v1/pipelines.
type Pipelines struct{ c *Client }

// PipelineDefinition is a definition as JSON, bytes as written: the order of
// jobs, matrix and inputs is part of it, so it is not a map.
type PipelineDefinition = json.RawMessage

// PipelineDoc is the pipeline document the definition routes answer. Version
// is the one shown, LatestVersion the newest. WebhookURL is only in the answer
// of the save that made the secret.
type PipelineDoc struct {
	Owner            string             `json:"owner"`
	Name             string             `json:"name"`
	Version          int                `json:"version"`
	LatestVersion    int                `json:"latestVersion"`
	CreatedBy        string             `json:"createdBy"`
	CreatedAtMs      int64              `json:"createdAtMs"`
	DefinitionSha256 string             `json:"definitionSha256"`
	Definition       PipelineDefinition `json:"definition"`
	Triggers         map[string]any     `json:"triggers"`
	Webhook          struct{ Set bool } `json:"webhook"`
	WebhookURL       string             `json:"webhookUrl"`
}

// PipelineSummary is one row of List.
type PipelineSummary struct {
	Name          string         `json:"name"`
	LatestVersion int            `json:"latestVersion"`
	UpdatedAtMs   int64          `json:"updatedAtMs"`
	Triggers      map[string]any `json:"triggers"`
}

// PipelineVersion is one row of Versions, oldest first.
type PipelineVersion struct {
	Version          int    `json:"version"`
	CreatedBy        string `json:"createdBy"`
	CreatedAtMs      int64  `json:"createdAtMs"`
	DefinitionSha256 string `json:"definitionSha256"`
}

// GetPipeline reads Name of Owner at Version; zero is the latest.
type GetPipeline struct {
	Owner   string
	Name    string
	Version int
}

// RunPipeline starts Owner's Pipeline by hand at Version (zero: the newest).
// Inputs are string values of the inputs the pipeline declares. Environment
// runs every job that names none in that environment; Clone instead in a
// per-run copy of it — one or the other.
type RunPipeline struct {
	Owner       string
	Pipeline    string
	Version     int
	Inputs      map[string]string
	Environment string
	Clone       string
}

// ListRuns reads Pipeline's runs of Owner, newest first; a zero Limit is the
// server's default (50, at most 200).
type ListRuns struct {
	Owner    string
	Pipeline string
	Limit    int
}

// GetRun reads one run by its id ("1760400000000-0a1b2c").
type GetRun struct {
	Owner    string
	Pipeline string
	Run      string
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
// failure or cancelled; Event is manual, schedule, webhook or pipeline. Environment is as requested: a string, {"clone": name}, or nil
// for the pipeline's own. Times are unix milliseconds, zero while unset.
type RunDoc struct {
	ID               string         `json:"id"`
	Number           int64          `json:"number"`
	Pipeline         string         `json:"pipeline"`
	Owner            string         `json:"owner"`
	PipelineVersion  int            `json:"pipelineVersion"`
	DefinitionSha256 string         `json:"definitionSha256"`
	Event            string         `json:"event"`
	Inputs           map[string]any `json:"inputs"`
	Environment      any            `json:"environment"`
	RequestedBy      string         `json:"requestedBy"`
	RequestedAtMs    int64          `json:"requestedAtMs"`
	State            string         `json:"state"`
	CancelRequested  bool           `json:"cancelRequested"`
	StartedAtMs      int64          `json:"startedAtMs"`
	FinishedAtMs     int64          `json:"finishedAtMs"`
	Detail           string         `json:"detail"`
	Jobs             []RunJob       `json:"jobs"`
	// Definition is the version's, only when reading one run and while it exists.
	Definition PipelineDefinition `json:"definition"`
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
	Home         map[string]any `json:"home"`
	Saved        map[string]any `json:"saved"`
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

func pipelinePath(owner string, rest ...string) string {
	out := "/v1/pipelines/" + seg(owner)
	for _, r := range rest {
		out += "/" + seg(r)
	}
	return out
}

// Create saves d (its name is the pipeline's) as version 1. A webhook
// trigger's URL is in WebhookURL, this once. A taken name is a 409.
func (p *Pipelines) Create(owner string, d PipelineDefinition) (*PipelineDoc, error) {
	var out PipelineDoc
	if err := p.c.call("POST", pipelinePath(owner), nil, d, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Update replaces the definition: a changed one is the next version, an equal
// one saves nothing. To roll back, send an old definition. A concurrent save
// is an *ApiError 409 ("changed; retry").
func (p *Pipelines) Update(owner, name string, d PipelineDefinition) (*PipelineDoc, error) {
	var out PipelineDoc
	if err := p.c.call("PUT", pipelinePath(owner, name), nil, d, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// List answers owner's pipelines by name.
func (p *Pipelines) List(owner string) ([]PipelineSummary, error) {
	var out []PipelineSummary
	if err := p.c.call("GET", pipelinePath(owner), nil, nil, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// Get answers one pipeline document, Definition included.
func (p *Pipelines) Get(req GetPipeline) (*PipelineDoc, error) {
	q := url.Values{}
	if req.Version > 0 {
		q.Set("version", fmt.Sprint(req.Version))
	}
	var out PipelineDoc
	if err := p.c.call("GET", pipelinePath(req.Owner, req.Name), q, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Versions answers the pipeline's versions, oldest first.
func (p *Pipelines) Versions(owner, name string) ([]PipelineVersion, error) {
	var out []PipelineVersion
	if err := p.c.call("GET", pipelinePath(owner, name, "versions"), nil, nil, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// Delete removes the pipeline and its versions; finished runs stay. A queued
// or running run is a 409.
func (p *Pipelines) Delete(owner, name string) error {
	return p.c.call("DELETE", pipelinePath(owner, name), nil, nil, nil)
}

// DeleteVersion removes one version; the latest, or one a queued or running
// run uses, is a 409.
func (p *Pipelines) DeleteVersion(owner, name string, version int) error {
	return p.c.call("DELETE", pipelinePath(owner, name, "versions", fmt.Sprint(version)), nil, nil, nil)
}

// RotateWebhook makes a new webhook secret and answers its URL, shown only
// here; the old URL stops at once.
func (p *Pipelines) RotateWebhook(owner, name string) (string, error) {
	var out struct {
		WebhookURL string `json:"webhookUrl"`
	}
	if err := p.c.call("POST", pipelinePath(owner, name, "webhook", "rotate"), nil, nil, &out); err != nil {
		return "", err
	}
	return out.WebhookURL, nil
}

// Run starts a run and answers it at once (202), queued. A version that
// cannot run is an *ApiError 422 whose Message lists every reason, and
// nothing is recorded.
func (p *Pipelines) Run(req RunPipeline) (*Run, error) {
	if req.Environment != "" && req.Clone != "" {
		return nil, fmt.Errorf("set Environment or Clone, not both")
	}
	body := toolArgs{}.num("version", int64(req.Version)).val("inputs", req.Inputs, req.Inputs != nil).
		str("environment", req.Environment).val("environment", map[string]string{"clone": req.Clone}, req.Clone != "")
	r := &Run{c: p.c}
	if err := p.c.call("POST", pipelinePath(req.Owner, req.Pipeline, "runs"), nil, map[string]any(body), &r.RunDoc); err != nil {
		return nil, err
	}
	return r, nil
}

// Runs answers the pipeline's runs, newest first.
func (p *Pipelines) Runs(req ListRuns) ([]*Run, error) {
	q := url.Values{}
	if req.Limit > 0 {
		q.Set("limit", fmt.Sprint(req.Limit))
	}
	var docs []RunDoc
	if err := p.c.call("GET", pipelinePath(req.Owner, req.Pipeline, "runs"), q, nil, &docs); err != nil {
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
	r := &Run{c: p.c}
	if err := p.c.call("GET", pipelinePath(req.Owner, req.Pipeline, "runs", req.Run), nil, nil, &r.RunDoc); err != nil {
		return nil, err
	}
	return r, nil
}

func (r *Run) path(tail string) string {
	return pipelinePath(r.Owner, r.Pipeline, "runs", r.ID) + tail
}

// Done reports whether the run has ended (success, failure or cancelled) as
// of the last read.
func (r *Run) Done() bool {
	return r.State == "success" || r.State == "failure" || r.State == "cancelled"
}

// Refresh re-reads the doc.
func (r *Run) Refresh() error {
	var d RunDoc
	if err := r.c.call("GET", r.path(""), nil, nil, &d); err != nil {
		return err
	}
	r.RunDoc = d
	return nil
}

// Cancel asks the run to stop: a queued run is cancelled at once, a running
// one once its worker has ended its sandboxes — Wait sees it reach cancelled.
func (r *Run) Cancel() error {
	var d RunDoc
	if err := r.c.call("POST", r.path("/cancel"), nil, nil, &d); err != nil {
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
	var page LogPage
	if err := r.c.call("GET", r.path(fmt.Sprintf("/jobs/%d/logs", index)), url.Values{"offset": {fmt.Sprint(offset)}}, nil, &page); err != nil {
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
