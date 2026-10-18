// Package kloudlite is the Kloudlite SDK for Go — stdlib only.
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client, and every repository is named by ONE "owner/name" string — the
// owner half being a person's handle or a team slug.
//
// No call takes two inputs by position. A method whose only input is the
// resource itself takes that one string (Get(repo)); everything else takes ONE
// request struct named for the call, so every argument is spelled by name:
//
//	kl := kloudlite.New(kloudlite.Options{BaseURL: "https://api.example.com", Token: "…"})
//	repo, err := kl.CodeRepos.Create(kloudlite.CreateRepo{Repo: "acme/payments"})
//
// The structs are the caller's vocabulary, not the wire's: the SDK builds the
// bodies, paths and queries the server reads (Snapshot travels as snapshot_id,
// URL as git, To as name).
package kloudlite

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const Version = "0.1.0"

// ApiError is a non-2xx answer: Status is the HTTP code, Message the server's text.
type ApiError struct {
	Status  int
	Message string
}

func (e *ApiError) Error() string { return fmt.Sprintf("%d: %s", e.Status, e.Message) }

// Ptr answers a pointer to v — for the few fields whose zero value is a real
// choice (CreateHubRepo.Private, UpdateRepo.Description), where nil means
// "not set".
func Ptr[T any](v T) *T { return &v }

type Client struct {
	CodeRepos *CodeRepos
	// ModelRepos and DataRepos live on the hub listener; their calls fail with a
	// plain error until Options.HubURL is given.
	ModelRepos   *HubRepos
	DataRepos    *HubRepos
	Workspaces   *Workspaces
	Environments *Environments
	Sessions     *Sessions
	Volumes      *Volumes
	Secrets      *Secrets
	Sandboxes    *Sandboxes
	Pipelines    *Pipelines

	baseURL string
	hubURL  string
	token   string
	http    *http.Client
}

// Options configures New. An empty Token means anonymous access to public
// data; HubURL (e.g. https://hub.example.com) is needed only for models and
// datasets; a zero Timeout means 30 s.
type Options struct {
	BaseURL string
	Token   string
	HubURL  string
	Timeout time.Duration
}

// New makes a client.
func New(opts Options) *Client {
	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}
	c := &Client{
		baseURL: strings.TrimRight(opts.BaseURL, "/"),
		hubURL:  strings.TrimRight(opts.HubURL, "/"),
		token:   opts.Token,
		http:    &http.Client{Timeout: timeout},
	}
	c.CodeRepos = &CodeRepos{c}
	c.ModelRepos = &HubRepos{c: c, kind: "model", plural: "models"}
	c.DataRepos = &HubRepos{c: c, kind: "dataset", plural: "datasets"}
	c.Workspaces = &Workspaces{c}
	c.Environments = &Environments{c}
	c.Sessions = &Sessions{c}
	c.Volumes = &Volumes{c}
	c.Secrets = &Secrets{c}
	c.Sandboxes = &Sandboxes{c}
	c.Pipelines = &Pipelines{c}
	return c
}

// Quota answers the ceiling and what is against it; an empty owner means the caller's own.
func (c *Client) Quota(owner string) (map[string]any, error) {
	q := url.Values{}
	if owner != "" {
		q.Set("owner", owner)
	}
	var out map[string]any
	err := c.call("GET", "/v1/quota", q, nil, &out)
	return out, err
}

// RequestOptions are the named halves of a raw round trip. Body (sent as
// JSON) and Out (decoded from the answer) may each be nil.
type RequestOptions struct {
	Query url.Values
	Body  any
	Out   any
}

// Request makes one HTTP round trip — the escape hatch under every module.
// method and path stay positional: they are the one obvious HTTP pair. Any
// non-2xx status is an *ApiError.
func (c *Client) Request(method, path string, opts RequestOptions) error {
	return c.call(method, path, opts.Query, opts.Body, opts.Out)
}

// HubRequest is the same round trip against the hub listener.
func (c *Client) HubRequest(method, path string, opts RequestOptions) error {
	return c.hub(method, path, opts.Query, opts.Body, opts.Out)
}

func (c *Client) call(method, path string, query url.Values, body, out any) error {
	return c.send(c.baseURL, method, path, query, body, out)
}

func (c *Client) hub(method, path string, query url.Values, body, out any) error {
	if c.hubURL == "" {
		return fmt.Errorf("this client has no hub URL; set Options.HubURL to reach models and datasets")
	}
	return c.send(c.hubURL, method, path, query, body, out)
}

func (c *Client) send(base, method, path string, query url.Values, body, out any) error {
	u := base + path
	if len(query) > 0 {
		u += "?" + query.Encode()
	}
	var rdr io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		rdr = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, u, rdr)
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", "kloudlite-sdk-go/"+Version)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return err
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return &ApiError{Status: resp.StatusCode, Message: errorText(raw)}
	}
	if out != nil && len(raw) > 0 {
		return json.Unmarshal(raw, out)
	}
	return nil
}

// CodeRepos is the code-repository module: /v1/repos. Every repo argument is one
// "owner/name" string; only owner-scoped calls (List) take a bare handle.
type CodeRepos struct{ c *Client }

type Repo struct {
	ID          string `json:"_id"`
	Owner       string `json:"owner"`
	Name        string `json:"name"`
	Public      bool   `json:"public"`
	Description string `json:"description"`
	CreatedBy   string `json:"created_by"`
	CreatedAt   int64  `json:"created_at"`
}

type Branch struct {
	Name    string `json:"name"`
	Oid     string `json:"oid"`
	Default bool   `json:"default"`
}

// FileChange is one file's worth of a commit. ContentBase64 carries the bytes
// (absent when Delete is set); a nil Executable keeps the mode the file has.
type FileChange struct {
	Path          string `json:"path"`
	ContentBase64 string `json:"contentBase64,omitempty"`
	Executable    *bool  `json:"executable,omitempty"`
	Delete        bool   `json:"delete,omitempty"`
}

// CreateRepo makes Repo ("owner/name"). An empty Visibility means private.
type CreateRepo struct {
	Repo        string
	Visibility  string
	Description string
}

// UpdateRepo changes what is set: a nil Description and an empty Visibility
// are left alone (Ptr("") clears the description).
type UpdateRepo struct {
	Repo        string
	Description *string
	Visibility  string
}

// ForkRepo copies Repo into To ("owner/name"); private unless Visibility says
// otherwise, whatever the source was.
type ForkRepo struct {
	Repo        string
	To          string
	Visibility  string
	Description string
}

// ImportRepo makes Repo from a PUBLIC git remote; URL is a strict "https://"
// URL (GitHub is one host among any).
type ImportRepo struct {
	Repo        string
	URL         string
	Visibility  string
	Description string
}

// DeleteBranch carries the head Oid the caller saw; the delete is refused if
// the branch moved.
type DeleteBranch struct {
	Repo   string
	Branch string
	Oid    string
}

// ProtectBranch sets (or with Remove, drops) the rule for Pattern.
type ProtectBranch struct {
	Repo     string
	Pattern  string
	Remove   bool
	NoForce  bool
	NoDelete bool
}

// ListPulls filters a repo's pulls; an empty State and a zero Limit mean the
// server's defaults.
type ListPulls struct {
	Repo  string
	State string
	Limit int
}

type GetPull struct {
	Repo   string
	Number int64
}

// OpenPull proposes Head onto Base. HeadRepo (another "owner/name") makes it a
// cross-repo pull: Head then names a branch of that fork.
type OpenPull struct {
	Repo        string
	Title       string
	Head        string
	Base        string
	HeadRepo    string
	Description string
}

type CommentOnPull struct {
	Repo   string
	Number int64
	Body   string
}

// MergePull's Strategy is fast-forward (the default when empty), squash, merge
// or rebase.
type MergePull struct {
	Repo     string
	Number   int64
	Strategy string
}

type ClosePull struct {
	Repo   string
	Number int64
}

type CompareRefs struct {
	Repo string
	Base string
	Head string
}

// CommitChanges is one commit of file changes; a non-empty Expect refuses the
// commit if the branch moved, and NewBranch lands it on a new branch instead.
type CommitChanges struct {
	Repo      string
	Branch    string
	Message   string
	Changes   []FileChange
	Expect    string
	NewBranch string
}

type GetSignature struct {
	Repo string
	SHA  string
}

func seg(s string) string { return url.PathEscape(s) }

// splitRepo turns "owner/name" into its two halves, refused client-side when
// malformed — a shape the server would only ever 404.
func splitRepo(repo string) (string, string, error) {
	owner, name, ok := strings.Cut(repo, "/")
	if !ok || owner == "" || name == "" || strings.Contains(name, "/") {
		return "", "", fmt.Errorf("a repository is named owner/name, got %q", repo)
	}
	return owner, name, nil
}

func repoPath(repo, tail string) (string, error) {
	owner, name, err := splitRepo(repo)
	if err != nil {
		return "", err
	}
	return "/v1/repos/" + seg(owner) + "/" + seg(name) + tail, nil
}

// newRepoBody is the create/fork/import wire shape: the destination split back
// into the two fields the server reads.
func newRepoBody(repo, visibility, description string) (map[string]string, error) {
	owner, name, err := splitRepo(repo)
	if err != nil {
		return nil, err
	}
	if visibility == "" {
		visibility = "private"
	}
	return map[string]string{
		"owner": owner, "name": name,
		"visibility": visibility, "description": description,
	}, nil
}

func (r *CodeRepos) Create(req CreateRepo) (*Repo, error) {
	body, err := newRepoBody(req.Repo, req.Visibility, req.Description)
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.call("POST", "/v1/repos", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *CodeRepos) List(owner string) ([]Repo, error) {
	var out []Repo
	err := r.c.call("GET", "/v1/repos", url.Values{"owner": {owner}}, nil, &out)
	return out, err
}

func (r *CodeRepos) Get(repo string) (*Repo, error) {
	p, err := repoPath(repo, "")
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.call("GET", p, nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *CodeRepos) Update(req UpdateRepo) error {
	p, err := repoPath(req.Repo, "")
	if err != nil {
		return err
	}
	// The wire shape: a nil description is absent, an empty one clears it.
	body := struct {
		Description *string `json:"description,omitempty"`
		Visibility  string  `json:"visibility,omitempty"`
	}{req.Description, req.Visibility}
	return r.c.call("PATCH", p, nil, body, nil)
}

func (r *CodeRepos) Delete(repo string) error {
	p, err := repoPath(repo, "")
	if err != nil {
		return err
	}
	return r.c.call("DELETE", p, nil, nil, nil)
}

// Fork makes a new repo To copied from Repo: objects and refs, never its pull
// requests or settings. Private by default, whatever the source was.
func (r *CodeRepos) Fork(req ForkRepo) (*Repo, error) {
	p, err := repoPath(req.Repo, "/fork")
	if err != nil {
		return nil, err
	}
	body, err := newRepoBody(req.To, req.Visibility, req.Description)
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.call("POST", p, nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ImportGit makes a new repo whose contents are fetched from a PUBLIC git
// remote. The server answers 202: the repo exists but the fetch runs in the
// background; poll ImportStatus.
func (r *CodeRepos) ImportGit(req ImportRepo) (*Repo, error) {
	body, err := newRepoBody(req.Repo, req.Visibility, req.Description)
	if err != nil {
		return nil, err
	}
	body["git"] = req.URL
	var out Repo
	if err := r.c.call("POST", "/v1/repos/import", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ImportStatus answers {from, state: queued|running|done|failed, detail?}.
func (r *CodeRepos) ImportStatus(repo string) (map[string]any, error) {
	p, err := repoPath(repo, "/import")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.call("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) Branches(repo string) ([]Branch, error) {
	p, err := repoPath(repo, "/branches")
	if err != nil {
		return nil, err
	}
	var out []Branch
	err = r.c.call("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) DeleteBranch(req DeleteBranch) error {
	p, err := repoPath(req.Repo, "/branches/"+seg(req.Branch))
	if err != nil {
		return err
	}
	return r.c.call("DELETE", p, url.Values{"oid": {req.Oid}}, nil, nil)
}

func (r *CodeRepos) Protection(repo string) (any, error) {
	p, err := repoPath(repo, "/protection")
	if err != nil {
		return nil, err
	}
	var out any
	err = r.c.call("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) Protect(req ProtectBranch) error {
	p, err := repoPath(req.Repo, "/protection")
	if err != nil {
		return err
	}
	body := struct {
		Pattern  string `json:"pattern"`
		Remove   bool   `json:"remove,omitempty"`
		NoForce  bool   `json:"no_force"`
		NoDelete bool   `json:"no_delete"`
	}{req.Pattern, req.Remove, req.NoForce, req.NoDelete}
	return r.c.call("POST", p, nil, body, nil)
}

func (r *CodeRepos) Pulls(req ListPulls) ([]map[string]any, error) {
	p, err := repoPath(req.Repo, "/pulls")
	if err != nil {
		return nil, err
	}
	q := url.Values{}
	if req.State != "" {
		q.Set("state", req.State)
	}
	if req.Limit > 0 {
		q.Set("limit", fmt.Sprint(req.Limit))
	}
	var out []map[string]any
	err = r.c.call("GET", p, q, nil, &out)
	return out, err
}

func (r *CodeRepos) Pull(req GetPull) (map[string]any, error) {
	p, err := repoPath(req.Repo, fmt.Sprintf("/pulls/%d", req.Number))
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.call("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) OpenPull(req OpenPull) (map[string]any, error) {
	p, err := repoPath(req.Repo, "/pulls")
	if err != nil {
		return nil, err
	}
	if req.HeadRepo != "" {
		if _, _, err := splitRepo(req.HeadRepo); err != nil {
			return nil, err
		}
	}
	body := struct {
		Title       string `json:"title"`
		Head        string `json:"head"`
		Base        string `json:"base"`
		HeadRepo    string `json:"head_repo,omitempty"`
		Description string `json:"description"`
	}{req.Title, req.Head, req.Base, req.HeadRepo, req.Description}
	var out map[string]any
	err = r.c.call("POST", p, nil, body, &out)
	return out, err
}

func (r *CodeRepos) Comment(req CommentOnPull) error {
	p, err := repoPath(req.Repo, fmt.Sprintf("/pulls/%d/comments", req.Number))
	if err != nil {
		return err
	}
	return r.c.call("POST", p, nil, map[string]string{"body": req.Body}, nil)
}

// Merge asks for the merge as a job (the server answers 202).
func (r *CodeRepos) Merge(req MergePull) error {
	p, err := repoPath(req.Repo, fmt.Sprintf("/pulls/%d/merge", req.Number))
	if err != nil {
		return err
	}
	strategy := req.Strategy
	if strategy == "" {
		strategy = "fast-forward"
	}
	return r.c.call("POST", p, url.Values{"strategy": {strategy}}, nil, nil)
}

func (r *CodeRepos) Close(req ClosePull) error {
	p, err := repoPath(req.Repo, fmt.Sprintf("/pulls/%d/close", req.Number))
	if err != nil {
		return err
	}
	return r.c.call("POST", p, nil, nil, nil)
}

func (r *CodeRepos) Compare(req CompareRefs) (map[string]any, error) {
	p, err := repoPath(req.Repo, "/compare")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.call("GET", p, url.Values{"base": {req.Base}, "head": {req.Head}}, nil, &out)
	return out, err
}

func (r *CodeRepos) Commit(req CommitChanges) (map[string]any, error) {
	p, err := repoPath(req.Repo, "/commits")
	if err != nil {
		return nil, err
	}
	body := struct {
		Branch    string       `json:"branch"`
		Message   string       `json:"message"`
		Changes   []FileChange `json:"changes"`
		Expect    string       `json:"expect,omitempty"`
		NewBranch string       `json:"newBranch,omitempty"`
	}{req.Branch, req.Message, req.Changes, req.Expect, req.NewBranch}
	var out map[string]any
	err = r.c.call("POST", p, nil, body, &out)
	return out, err
}

func (r *CodeRepos) Signature(req GetSignature) (map[string]any, error) {
	p, err := repoPath(req.Repo, "/commits/"+seg(req.SHA)+"/signature")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.call("GET", p, nil, nil, &out)
	return out, err
}

// HubRepos is model and dataset repositories, on the hub listener (Hugging
// Face Hub API). Management level: create, read, refs, visibility, delete —
// files move with git/huggingface_hub against the same hub URL. Every repo
// argument is one "owner/name" string, exactly as in CodeRepos.
type HubRepos struct {
	c      *Client
	kind   string
	plural string
}

// CreateHubRepo makes Repo; a nil Private means private (Ptr(false) for a
// public one), never a server default.
type CreateHubRepo struct {
	Repo    string
	Private *bool
}

type SetPrivate struct {
	Repo    string
	Private bool
}

// ImportHubRepo makes Repo from a PUBLIC git remote — large files included,
// through git-lfs; URL is a strict "https://" URL (the Hugging Face Hub is one
// host among any). A nil Private means private.
type ImportHubRepo struct {
	Repo    string
	URL     string
	Private *bool
}

// private resolves the nil-means-private default.
func private(p *bool) bool { return p == nil || *p }

func (h *HubRepos) body(repo string, extra map[string]any) (map[string]any, error) {
	owner, name, err := splitRepo(repo)
	if err != nil {
		return nil, err
	}
	out := map[string]any{"name": name, "organization": owner, "type": h.kind}
	for k, v := range extra {
		out[k] = v
	}
	return out, nil
}

func (h *HubRepos) path(repo, tail string) (string, error) {
	owner, name, err := splitRepo(repo)
	if err != nil {
		return "", err
	}
	return "/api/" + h.plural + "/" + seg(owner) + "/" + seg(name) + tail, nil
}

// Create sends private explicitly rather than trusting a server default.
func (h *HubRepos) Create(req CreateHubRepo) (map[string]any, error) {
	body, err := h.body(req.Repo, map[string]any{"private": private(req.Private)})
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.hub("POST", "/api/repos/create", nil, body, &out)
	return out, err
}

func (h *HubRepos) Get(repo string) (map[string]any, error) {
	p, err := h.path(repo, "")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.hub("GET", p, nil, nil, &out)
	return out, err
}

// Refs answers branches and tags, as the hub spells them.
func (h *HubRepos) Refs(repo string) (map[string]any, error) {
	p, err := h.path(repo, "/refs")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.hub("GET", p, nil, nil, &out)
	return out, err
}

func (h *HubRepos) SetPrivate(req SetPrivate) error {
	p, err := h.path(req.Repo, "/settings")
	if err != nil {
		return err
	}
	return h.c.hub("PUT", p, nil, map[string]any{"private": req.Private}, nil)
}

func (h *HubRepos) Delete(repo string) error {
	body, err := h.body(repo, nil)
	if err != nil {
		return err
	}
	return h.c.hub("DELETE", "/api/repos/delete", nil, body, nil)
}

// ImportGit answers 202; poll ImportStatus.
func (h *HubRepos) ImportGit(req ImportHubRepo) (map[string]any, error) {
	p, err := h.path(req.Repo, "/import")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.hub("POST", p, nil, map[string]any{"from": req.URL, "private": private(req.Private)}, &out)
	return out, err
}

// ImportStatus answers {from, state: queued|running|done|failed, detail?}.
func (h *HubRepos) ImportStatus(repo string) (map[string]any, error) {
	p, err := h.path(repo, "/import")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.hub("GET", p, nil, nil, &out)
	return out, err
}

// Workspaces is btrfs-backed dev workspaces, over /v1/workspaces. A workspace
// is named like an environment — "name" (personal), "team/name" (a team's) or
// its id — and travels as path segment name plus ?owner=team, which the
// server folds back. Pushes cut snapshots, kept on the workspace's VOLUME —
// see Volumes for history, restore targets and deletion.
type Workspaces struct{ c *Client }

// CreateWorkspace carries what a create takes. Workspace is "name" or
// "team/name"; Repo seeds from a PLATFORM repo ("owner/name", with Branch);
// Packages are nixpkgs attributes, optionally pinned ("attr@version"). An
// empty Image means the platform default.
type CreateWorkspace struct {
	Workspace string
	QuotaGb   int64
	Image     string
	Repo      string
	Branch    string
	Packages  []string
	// TtlSecs (600..=2592000) deletes the workspace after that many seconds
	// unused; 0 is never.
	TtlSecs int64
	// Env maps a variable to a literal value or a "${{ secrets.NAME }}" reference.
	Env map[string]string
}

type PushWorkspace struct {
	Workspace string
	Message   string
}

// CloneWorkspace makes To from Workspace's state; Task is optional.
type CloneWorkspace struct {
	Workspace string
	To        string
	Task      string
}

// RestoreWorkspace grafts a new workspace To onto a PUSHED Snapshot — also one
// whose workspace is long gone. Absent overrides mean "whatever the snapshot
// froze".
type RestoreWorkspace struct {
	Snapshot string
	To       string
	Image    string
	Packages []string
	QuotaGb  int64
}

type SetPackages struct {
	Workspace string
	Packages  []string
}

func (w *Workspaces) Create(req CreateWorkspace) (map[string]any, error) {
	team, name, err := owned(req.Workspace)
	if err != nil {
		return nil, err
	}
	packages := req.Packages
	if packages == nil {
		packages = []string{}
	}
	body := struct {
		Name     string            `json:"name"`
		QuotaGb  int64             `json:"quota_gb"`
		Team     string            `json:"team,omitempty"`
		Image    string            `json:"image,omitempty"`
		Repo     string            `json:"repo,omitempty"`
		Branch   string            `json:"branch,omitempty"`
		Packages []string          `json:"packages"`
		TtlSecs  int64             `json:"ttl_secs,omitempty"`
		Env      map[string]string `json:"env,omitempty"`
	}{name, req.QuotaGb, team, req.Image, req.Repo, req.Branch, packages, req.TtlSecs, req.Env}
	var out map[string]any
	err = w.c.call("POST", "/v1/workspaces", nil, body, &out)
	return out, err
}

// SetTTL sets the seconds the workspace may sit unused before it is deleted
// (600..=2592000); nil clears it. Not for a sandbox or a bench.
func (w *Workspaces) SetTTL(workspace string, ttlSecs *int64) (map[string]any, error) {
	var out map[string]any
	err := w.at("PUT", workspace, "/ttl", map[string]*int64{"ttl_secs": ttlSecs}, &out)
	return out, err
}

// SetEnv replaces the whole env (literals or "${{ secrets.NAME }}" references); it
// takes effect on the next start.
func (w *Workspaces) SetEnv(workspace string, env map[string]string) (map[string]any, error) {
	var out map[string]any
	err := w.at("PUT", workspace, "/env", map[string]map[string]string{"env": env}, &out)
	return out, err
}

// List answers the caller's workspaces; team scopes to their workspaces in
// that team.
func (w *Workspaces) List(team string) ([]map[string]any, error) {
	q := url.Values{}
	if team != "" {
		q.Set("team", team)
	}
	var out []map[string]any
	err := w.c.call("GET", "/v1/workspaces", q, nil, &out)
	return out, err
}

func (w *Workspaces) Get(workspace string) (map[string]any, error) {
	var out map[string]any
	err := w.at("GET", workspace, "", nil, &out)
	return out, err
}

// Delete runs the worktree finalizer: the volume survives while a pushed
// snapshot references it.
func (w *Workspaces) Delete(workspace string) error {
	return w.at("DELETE", workspace, "", nil, nil)
}

func (w *Workspaces) Start(workspace string) error {
	return w.at("POST", workspace, "/start", nil, nil)
}

// Stop cuts a sync point and tears the pod down — seconds, never waiting on a
// replica.
func (w *Workspaces) Stop(workspace string) error {
	return w.at("POST", workspace, "/stop", nil, nil)
}

// Push cuts a SNAPSHOT of the working copy — the one durable, restorable state.
func (w *Workspaces) Push(req PushWorkspace) (map[string]any, error) {
	var body any
	if req.Message != "" {
		body = map[string]string{"message": req.Message}
	}
	var out map[string]any
	err := w.at("POST", req.Workspace, "/push", body, &out)
	return out, err
}

// Clone makes a new workspace from this one's state, cut at the moment of the
// request; the answer's based_on names exactly what it grafted onto.
func (w *Workspaces) Clone(req CloneWorkspace) (map[string]any, error) {
	body := map[string]string{"name": req.To}
	if req.Task != "" {
		body["task"] = req.Task
	}
	var out map[string]any
	err := w.at("POST", req.Workspace, "/clone", body, &out)
	return out, err
}

func (w *Workspaces) Restore(req RestoreWorkspace) (map[string]any, error) {
	body := struct {
		Name       string   `json:"name"`
		SnapshotID string   `json:"snapshot_id"`
		Image      string   `json:"image,omitempty"`
		Packages   []string `json:"packages,omitempty"`
		QuotaGb    int64    `json:"quota_gb,omitempty"`
	}{req.To, req.Snapshot, req.Image, req.Packages, req.QuotaGb}
	var out map[string]any
	err := w.c.call("POST", "/v1/workspaces/restore", nil, body, &out)
	return out, err
}

// SetPackages replaces the package list; pinned entries are locked before
// anything is written.
func (w *Workspaces) SetPackages(req SetPackages) error {
	return w.at("PATCH", req.Workspace, "", map[string]any{"packages": req.Packages}, nil)
}

// UpdatePackages re-resolves every pinned package — the only call that moves
// existing locks.
func (w *Workspaces) UpdatePackages(workspace string) error {
	return w.at("POST", workspace, "/packages/update", nil, nil)
}

// Tools answers the workspace's tool-server address and token — to the owner
// alone.
func (w *Workspaces) Tools(workspace string) (map[string]any, error) {
	var out map[string]any
	err := w.at("GET", workspace, "/tools", nil, &out)
	return out, err
}

// Ide answers an *Ide bound to this workspace's tool server. The address is a
// pod IP: reachable from inside the fleet (a bench); from a laptop, tunnel
// with `kl-connect ws ide` and use
// NewIde(IdeOptions{URL: "http://127.0.0.1:7788", Token: token}) instead.
func (w *Workspaces) Ide(workspace string) (*Ide, error) {
	doc, err := w.Tools(workspace)
	if err != nil {
		return nil, err
	}
	addr, _ := doc["address"].(string)
	if addr == "" {
		return nil, fmt.Errorf("the tools answer named no address")
	}
	token, _ := doc["token"].(string)
	return NewIde(IdeOptions{URL: "http://" + addr, Token: token}), nil
}

func (w *Workspaces) at(method, workspace, tail string, body, out any) error {
	return w.c.named(method, "/v1/workspaces/", workspace, tail, body, out)
}

// Ide is a workspace's tool server (`kl ide serve`): the full tool set over
// plain HTTP — List answers every tool with its JSON schema (the authority on
// arguments) and Call runs one with an args map; nil-valued and absent keys
// mean "the tool's own default". The typed helpers cover every tool; in their
// structs a zero field is left out, so it too means the tool's default.
type Ide struct {
	url   string
	token string
	http  *http.Client
}

// IdeOptions configures NewIde; a zero Timeout means 630 s.
type IdeOptions struct {
	URL     string
	Token   string
	Timeout time.Duration
}

func NewIde(opts IdeOptions) *Ide {
	timeout := opts.Timeout
	if timeout == 0 {
		// exec may legitimately run for ten minutes; the client outlasts it.
		timeout = 630 * time.Second
	}
	return &Ide{
		url:   strings.TrimRight(opts.URL, "/"),
		token: opts.Token,
		http:  &http.Client{Timeout: timeout},
	}
}

// ReadFiles pages text files: one Path, or several as Paths.
type ReadFiles struct {
	Path   string
	Paths  []string
	Offset int
	Limit  int
}

type WriteFile struct {
	Path    string
	Content string
}

// Edit is one exact replacement: Old must occur exactly once unless
// ReplaceAll is set.
type Edit struct {
	Old        string `json:"old"`
	New        string `json:"new"`
	ReplaceAll bool   `json:"replace_all,omitempty"`
}

type FileEdits struct {
	Path  string `json:"path"`
	Edits []Edit `json:"edits"`
}

// EditFiles is Path+Edits for one file, Files for several — atomic across the
// whole call.
type EditFiles struct {
	Path  string
	Edits []Edit
	Files []FileEdits
}

type ApplyPatch struct {
	Diff string
	Cwd  string
}

type GlobFiles struct {
	Pattern string
	Cwd     string
}

type GrepFiles struct {
	Pattern    string
	Cwd        string
	Glob       string
	Mode       string
	Context    int
	IgnoreCase bool
	Max        int
}

// Exec runs Cmd — a shell string or an argv []string. With Detach the answer
// is {id}, the Process for ProcessOutput/ProcessKill.
type Exec struct {
	Cmd       any
	Cwd       string
	Env       map[string]string
	TimeoutMs int64
	Head      int
	Tail      int
	Quiet     bool
	Detach    bool
}

// ReadProcess pages a process's output from byte offsets (0 = the start);
// Since addresses stdout, SinceErr stderr.
type ReadProcess struct {
	Process  string
	Since    int64
	SinceErr int64
}

type WriteProcess struct {
	Process string
	Data    string
}

type KillProcess struct {
	Process string
	Signal  string
}

// StartWatch watches Paths (file changes) or Cmd (its output lines), filtered
// by the Pattern regex; Once ends it at the first event.
type StartWatch struct {
	Paths   []string
	Cmd     any
	Pattern string
	Once    bool
}

type PollWatch struct {
	Watch string
	Since int64
}

// BuildContainer builds on the owner's builder and pushes each of Tags
// ("name[:tag]").
type BuildContainer struct {
	Tags      []string
	Context   string
	File      string
	BuildArgs []string
	Platform  string
	NoCache   bool
	TimeoutMs int64
	Head      int
	Tail      int
}

// PushContainer copies an image the registry already holds, Src, to Dst.
type PushContainer struct {
	Src string
	Dst []string
}

// toolArgs builds a tool's argument map, leaving out zero values so they mean
// "the tool's own default".
type toolArgs map[string]any

func (a toolArgs) str(k, v string) toolArgs {
	if v != "" {
		a[k] = v
	}
	return a
}

func (a toolArgs) num(k string, v int64) toolArgs {
	if v != 0 {
		a[k] = v
	}
	return a
}

func (a toolArgs) flag(k string, v bool) toolArgs {
	if v {
		a[k] = v
	}
	return a
}

func (a toolArgs) val(k string, v any, present bool) toolArgs {
	if present {
		a[k] = v
	}
	return a
}

// List answers every tool, with its JSON schema.
func (i *Ide) List() (any, error) {
	return i.send("GET", "/tools", nil)
}

// Call runs one tool by name with the arguments its schema names.
func (i *Ide) Call(tool string, args map[string]any) (any, error) {
	if args == nil {
		args = map[string]any{}
	}
	return i.send("POST", "/tools/"+seg(tool), args)
}

func (i *Ide) Read(req ReadFiles) (any, error) {
	a := toolArgs{}.str("path", req.Path).val("paths", req.Paths, req.Paths != nil).
		num("offset", int64(req.Offset)).num("limit", int64(req.Limit))
	return i.Call("read", a)
}

// Write creates or overwrites a file atomically.
func (i *Ide) Write(req WriteFile) (any, error) {
	return i.Call("write", map[string]any{"path": req.Path, "content": req.Content})
}

func (i *Ide) Edit(req EditFiles) (any, error) {
	a := toolArgs{}.str("path", req.Path).val("edits", req.Edits, req.Edits != nil).
		val("files", req.Files, req.Files != nil)
	return i.Call("edit", a)
}

// Patch applies a unified diff; it never applies in part.
func (i *Ide) Patch(req ApplyPatch) (any, error) {
	return i.Call("patch", toolArgs{"diff": req.Diff}.str("cwd", req.Cwd))
}

// Glob lists the files matching a pattern, newest first, gitignore respected.
func (i *Ide) Glob(req GlobFiles) (any, error) {
	return i.Call("glob", toolArgs{"pattern": req.Pattern}.str("cwd", req.Cwd))
}

// Grep searches with a regex.
func (i *Ide) Grep(req GrepFiles) (any, error) {
	a := toolArgs{"pattern": req.Pattern}.str("cwd", req.Cwd).str("glob", req.Glob).
		str("mode", req.Mode).num("context", int64(req.Context)).
		flag("ignore_case", req.IgnoreCase).num("max", int64(req.Max))
	return i.Call("grep", a)
}

func (i *Ide) Exec(req Exec) (any, error) {
	a := toolArgs{"cmd": req.Cmd}.str("cwd", req.Cwd).val("env", req.Env, req.Env != nil).
		num("timeout_ms", req.TimeoutMs).num("head", int64(req.Head)).num("tail", int64(req.Tail)).
		flag("quiet", req.Quiet).flag("detach", req.Detach)
	return i.Call("exec", a)
}

func (i *Ide) ProcessList() (any, error) { return i.Call("process_list", nil) }

// ProcessOutput always sends both offsets — 0 is the start, and also the
// tool's default.
func (i *Ide) ProcessOutput(req ReadProcess) (any, error) {
	return i.Call("process_output", map[string]any{"id": req.Process, "since": req.Since, "since_err": req.SinceErr})
}

func (i *Ide) ProcessWrite(req WriteProcess) (any, error) {
	return i.Call("process_write", map[string]any{"id": req.Process, "data": req.Data})
}

func (i *Ide) ProcessKill(req KillProcess) (any, error) {
	return i.Call("process_kill", toolArgs{"id": req.Process}.str("signal", req.Signal))
}

// Watch answers {id}, the watch for WatchPoll/WatchStop.
func (i *Ide) Watch(req StartWatch) (any, error) {
	a := toolArgs{}.val("paths", req.Paths, req.Paths != nil).val("cmd", req.Cmd, req.Cmd != nil).
		str("pattern", req.Pattern).flag("once", req.Once)
	return i.Call("watch", a)
}

func (i *Ide) WatchPoll(req PollWatch) (any, error) {
	return i.Call("watch_poll", map[string]any{"id": req.Watch, "since": req.Since})
}

func (i *Ide) WatchStop(watch string) (any, error) {
	return i.Call("watch_stop", map[string]any{"id": watch})
}

// ContainerBuild waits for the build (TimeoutMs, at most ten minutes).
func (i *Ide) ContainerBuild(req BuildContainer) (any, error) {
	a := toolArgs{"tags": req.Tags}.str("context", req.Context).str("file", req.File).
		val("build_args", req.BuildArgs, req.BuildArgs != nil).str("platform", req.Platform).
		flag("no_cache", req.NoCache).num("timeout_ms", req.TimeoutMs).
		num("head", int64(req.Head)).num("tail", int64(req.Tail))
	return i.Call("container_build", a)
}

// ContainerPush is a registry-side copy — nothing is rebuilt.
func (i *Ide) ContainerPush(req PushContainer) (any, error) {
	return i.Call("container_push", map[string]any{"src": req.Src, "dst": req.Dst})
}

func (i *Ide) send(method, path string, body map[string]any) (any, error) {
	var rdr io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		rdr = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, i.url+path, rdr)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "kloudlite-sdk-go/"+Version)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if i.token != "" {
		req.Header.Set("Authorization", "Bearer "+i.token)
	}
	resp, err := i.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return nil, &ApiError{Status: resp.StatusCode, Message: errorText(raw)}
	}
	if len(raw) == 0 {
		return nil, nil
	}
	var out any
	err = json.Unmarshal(raw, &out)
	return out, err
}

// Service is one container of an environment. Mounts are folders of the
// environment's own disk.
type Service struct {
	Name    string            `json:"name"`
	Image   string            `json:"image"`
	Ports   []int             `json:"ports"`
	Env     map[string]string `json:"env"`
	Command []string          `json:"command"`
	Mounts  []Mount           `json:"mounts"`
}

type Mount struct {
	Folder string `json:"folder"`
	Path   string `json:"path"`
}

// Environments are multi-service environments over /v1/environments: a set of
// containers sharing one disk. An environment is named by ONE string — "name"
// for the caller's own, "team/name" for a team's — or by its id (env-…).
type Environments struct{ c *Client }

// CreateEnvironment's optional halves: Snapshot grows the environment from a
// pushed snapshot, whose services then are the default; zero fields mean
// "not set".
type CreateEnvironment struct {
	Environment string
	Services    []Service
	QuotaGb     int64
	Snapshot    string
	// TtlSecs (600..=2592000) deletes the environment after that many seconds
	// unused; 0 is never.
	TtlSecs int64
}

type SetServices struct {
	Environment string
	Services    []Service
}

type PushEnvironment struct {
	Environment string
	Message     string
}

// CloneEnvironment makes To ("name" or "team/name"): a copy of the live disk,
// or with Snapshot of one of Environment's snapshots.
type CloneEnvironment struct {
	Environment string
	To          string
	Snapshot    string
	Services    []Service
	QuotaGb     int64
}

// RestoreEnvironment puts Environment back to one of its own snapshots. A
// persistent service the snapshot lacks is a 409 naming it; list it in
// Discard to accept losing its data.
type RestoreEnvironment struct {
	Environment string
	Snapshot    string
	Discard     []string
}

func (e *Environments) Create(req CreateEnvironment) (map[string]any, error) {
	body, err := envBody(req.Environment, req.Services, req.QuotaGb, req.Snapshot)
	if err != nil {
		return nil, err
	}
	if req.TtlSecs != 0 {
		body["ttl_secs"] = req.TtlSecs
	}
	if body["services"] == nil {
		body["services"] = []Service{}
	}
	var out map[string]any
	err = e.c.call("POST", "/v1/environments", nil, body, &out)
	return out, err
}

// List answers the caller's environments and their teams'; a non-empty owner
// narrows to one handle or team.
func (e *Environments) List(owner string) ([]map[string]any, error) {
	q := url.Values{}
	if owner != "" {
		q.Set("owner", owner)
	}
	var out []map[string]any
	err := e.c.call("GET", "/v1/environments", q, nil, &out)
	return out, err
}

func (e *Environments) Get(environment string) (map[string]any, error) {
	var out map[string]any
	err := e.at("GET", environment, "", nil, &out)
	return out, err
}

// SetServices replaces the WHOLE service list. A removed service keeps its
// folders, so adding it back finds its data.
func (e *Environments) SetServices(req SetServices) error {
	return e.at("PATCH", req.Environment, "", map[string]any{"services": fillServices(req.Services)}, nil)
}

func (e *Environments) Start(environment string) error {
	return e.at("POST", environment, "/start", nil, nil)
}

func (e *Environments) Stop(environment string) error {
	return e.at("POST", environment, "/stop", nil, nil)
}

// SetTTL sets the seconds the environment may sit unused before it is
// deleted (600..=2592000); nil clears it.
func (e *Environments) SetTTL(environment string, ttlSecs *int64) (map[string]any, error) {
	var out map[string]any
	err := e.at("PUT", environment, "/ttl", map[string]*int64{"ttl_secs": ttlSecs}, &out)
	return out, err
}

// Delete keeps the volume while a pushed snapshot references it.
func (e *Environments) Delete(environment string) error {
	return e.at("DELETE", environment, "", nil, nil)
}

// Push cuts a SNAPSHOT of every service's data — the one durable, restorable
// state.
func (e *Environments) Push(req PushEnvironment) (map[string]any, error) {
	var body any
	if req.Message != "" {
		body = map[string]string{"message": req.Message}
	}
	var out map[string]any
	err := e.at("POST", req.Environment, "/push", body, &out)
	return out, err
}

func (e *Environments) Clone(req CloneEnvironment) (map[string]any, error) {
	body, err := envBody(req.To, req.Services, req.QuotaGb, req.Snapshot)
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = e.at("POST", req.Environment, "/clone", body, &out)
	return out, err
}

// Restore restores services and data (202).
func (e *Environments) Restore(req RestoreEnvironment) error {
	discard := req.Discard
	if discard == nil {
		discard = []string{}
	}
	return e.at("POST", req.Environment, "/restore", map[string]any{"snapshot_id": req.Snapshot, "discard": discard}, nil)
}

func (e *Environments) at(method, environment, tail string, body, out any) error {
	return e.c.named(method, "/v1/environments/", environment, tail, body, out)
}

// named is the one shape every "name" / "team/name" resource travels in: the
// name as a path segment, the team as ?owner=, folded back by the server.
func (c *Client) named(method, prefix, s, tail string, body, out any) error {
	owner, name, err := owned(s)
	if err != nil {
		return err
	}
	q := url.Values{}
	if owner != "" {
		q.Set("owner", owner)
	}
	return c.call(method, prefix+seg(name)+tail, q, body, out)
}

// owned splits "name" into ("", name) and "team/name" into (team, name).
func owned(s string) (string, string, error) {
	i := strings.LastIndex(s, "/")
	if i < 0 {
		return "", s, nil
	}
	owner, name := s[:i], s[i+1:]
	if owner == "" || name == "" || strings.Contains(owner, "/") {
		return "", "", fmt.Errorf("expected name or team/name, got %q", s)
	}
	return owner, name, nil
}

func envBody(env string, services []Service, quotaGb int64, snapshot string) (map[string]any, error) {
	owner, name, err := owned(env)
	if err != nil {
		return nil, err
	}
	body := map[string]any{"name": name}
	if owner != "" {
		body["owner"] = owner
	}
	if services != nil {
		body["services"] = fillServices(services)
	}
	if quotaGb > 0 {
		body["quota_gb"] = quotaGb
	}
	if snapshot != "" {
		body["snapshot_id"] = snapshot
	}
	return body, nil
}

// fillServices replaces nil fields with empty ones: the server requires every
// field, and a nil slice or map would encode as null.
func fillServices(in []Service) []Service {
	out := make([]Service, len(in))
	for i, sv := range in {
		if sv.Ports == nil {
			sv.Ports = []int{}
		}
		if sv.Env == nil {
			sv.Env = map[string]string{}
		}
		if sv.Command == nil {
			sv.Command = []string{}
		}
		if sv.Mounts == nil {
			sv.Mounts = []Mount{}
		}
		out[i] = sv
	}
	return out
}

// Sessions are working sessions over /v1/sessions: some of YOUR workspaces in
// one team, connected to one environment. Many people's sessions connect to
// the same environment and collaborate through it; an intercept a session takes
// is environment-wide, and the session owns it — only it can release it, and
// removing the workspace or deleting the session releases it.
//
// A session is named like an environment — "name" (personal) or "team/name" —
// or by its id (ses-…); a workspace by its id, name or "team/name", sent
// verbatim.
type Sessions struct{ c *Client }

// CreateSession makes a new Session. Environment, when given, must be the
// session's team's: an id, a name or "team/name"; empty makes a session with
// none, to connect later.
type CreateSession struct {
	Session     string
	Environment string
	// TtlSecs (600..=2592000) deletes the session after that many seconds
	// unused; 0 is never.
	TtlSecs int64
}

// AddToSession puts Workspace in Session. A workspace is in at most one
// session; while in one it resolves that session's environment.
type AddToSession struct {
	Session   string
	Workspace string
}

// RemoveFromSession releases whatever Workspace intercepts in Session first.
type RemoveFromSession struct {
	Session   string
	Workspace string
}

// InterceptService sends everything that calls Service in the environment to
// Workspace (a member); Ports maps each service port to the workspace port
// that answers it ({8080: 3000}).
type InterceptService struct {
	Session   string
	Service   string
	Workspace string
	Ports     map[int]int
}

type ReleaseService struct {
	Session string
	Service string
}

func (s *Sessions) Create(req CreateSession) (map[string]any, error) {
	team, name, err := owned(req.Session)
	if err != nil {
		return nil, err
	}
	body := map[string]any{"name": name}
	if req.Environment != "" {
		body["environment"] = req.Environment
	}
	if req.TtlSecs != 0 {
		body["ttl_secs"] = req.TtlSecs
	}
	if team != "" {
		body["team"] = team
	}
	var out map[string]any
	err = s.c.call("POST", "/v1/sessions", nil, body, &out)
	return out, err
}

// SetTTL sets the seconds the session may sit unused before it is deleted
// (600..=2592000); nil clears it.
func (s *Sessions) SetTTL(session string, ttlSecs *int64) (map[string]any, error) {
	var out map[string]any
	err := s.at("PUT", session, "/ttl", map[string]*int64{"ttl_secs": ttlSecs}, &out)
	return out, err
}

func (s *Sessions) List(team string) ([]map[string]any, error) {
	q := url.Values{}
	if team != "" {
		q.Set("team", team)
	}
	var out []map[string]any
	err := s.c.call("GET", "/v1/sessions", q, nil, &out)
	return out, err
}

// Get answers the session with its workspaces and the intercepts it holds.
func (s *Sessions) Get(session string) (map[string]any, error) {
	var out map[string]any
	err := s.at("GET", session, "", nil, &out)
	return out, err
}

// Delete releases its intercepts and lets its workspaces go — never deletes a
// workspace.
func (s *Sessions) Delete(session string) error { return s.at("DELETE", session, "", nil, nil) }

// Connect sets, switches (environment non-nil: an id, a name or "team/name")
// or clears (nil) the session's environment. Its intercepts in the old
// environment are released first; deleting an environment disconnects its
// sessions the same way.
func (s *Sessions) Connect(session string, environment *string) (map[string]any, error) {
	var out map[string]any
	err := s.at("PUT", session, "/environment", map[string]*string{"environment": environment}, &out)
	return out, err
}

func (s *Sessions) Add(req AddToSession) (map[string]any, error) {
	var out map[string]any
	err := s.at("POST", req.Session, "/workspaces", map[string]string{"workspace": req.Workspace}, &out)
	return out, err
}

func (s *Sessions) Remove(req RemoveFromSession) error {
	return s.at("DELETE", req.Session, "/workspaces/"+seg(req.Workspace), nil, nil)
}

func (s *Sessions) Intercept(req InterceptService) (map[string]any, error) {
	pm := make([]map[string]int, 0, len(req.Ports))
	for sp, wp := range req.Ports {
		pm = append(pm, map[string]int{"service": sp, "workspace": wp})
	}
	var out map[string]any
	err := s.at("POST", req.Session, "/intercepts", map[string]any{"service": req.Service, "workspace": req.Workspace, "ports": pm}, &out)
	return out, err
}

func (s *Sessions) Release(req ReleaseService) error {
	return s.at("DELETE", req.Session, "/intercepts/"+seg(req.Service), nil, nil)
}

func (s *Sessions) at(method, session, tail string, body, out any) error {
	return s.c.named(method, "/v1/sessions/", session, tail, body, out)
}

// Volumes is the btrfs volumes behind workspaces and environments: snapshot
// history, restore targets, and the two deletes — over /v1/volumes.
type Volumes struct{ c *Client }

// ListVolumes filters by Kind ("workspace" or "environment") and Owner (a
// handle or a team slug); empty fields mean no filter.
type ListVolumes struct {
	Kind  string
	Owner string
}

// DeleteSnapshot is refused (409) for a sync point or a running worktree's
// base; deleting a detached volume's last snapshot deletes the volume.
type DeleteSnapshot struct {
	Volume   string
	Snapshot string
}

func (v *Volumes) List(req ListVolumes) ([]map[string]any, error) {
	q := url.Values{}
	if req.Kind != "" {
		q.Set("kind", req.Kind)
	}
	if req.Owner != "" {
		q.Set("owner", req.Owner)
	}
	var out []map[string]any
	err := v.c.call("GET", "/v1/volumes", q, nil, &out)
	return out, err
}

// History answers the chain of PUSHED snapshots, newest first — each row a
// restore target.
func (v *Volumes) History(volume string) ([]map[string]any, error) {
	var out []map[string]any
	err := v.c.call("GET", "/v1/volumes/"+seg(volume)+"/history", nil, nil, &out)
	return out, err
}

func (v *Volumes) Refs(volume string) (map[string]any, error) {
	var out map[string]any
	err := v.c.call("GET", "/v1/volumes/"+seg(volume)+"/refs", nil, nil, &out)
	return out, err
}

func (v *Volumes) DeleteSnapshot(req DeleteSnapshot) error {
	return v.c.call("DELETE", "/v1/volumes/"+seg(req.Volume)+"/snapshots/"+seg(req.Snapshot), nil, nil, nil)
}

// Delete takes a DETACHED volume with all its snapshots; refused while a
// working copy remains.
func (v *Volumes) Delete(volume string) error {
	return v.c.call("DELETE", "/v1/volumes/"+seg(volume), nil, nil, nil)
}

// The server answers errors as {"error": …} or plain text; take either.
func errorText(raw []byte) string {
	var e struct {
		Error string `json:"error"`
	}
	if json.Unmarshal(raw, &e) == nil && e.Error != "" {
		return e.Error
	}
	if t := strings.TrimSpace(string(raw)); t != "" {
		return t
	}
	return "request failed"
}
