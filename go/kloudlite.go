// Package kloudlite is the Kloudlite SDK for Go — stdlib only.
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client, and every repository is named by ONE "owner/name" string — the
// owner half being a person's handle or a team slug:
//
//	kl := kloudlite.New("https://api.example.com", "…")
//	repo, err := kl.CodeRepos.Create("acme/payments", kloudlite.RepoOpts{})
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

type Client struct {
	CodeRepos *CodeRepos
	// ModelRepos and DataRepos live on the hub listener; their calls fail with a
	// plain error until a hub URL is given (New's hubURL, or NewWithHub).
	ModelRepos *HubRepos
	DataRepos  *HubRepos

	baseURL string
	hubURL  string
	token   string
	http    *http.Client
}

// New makes a client. token may be empty for anonymous access to public data.
func New(baseURL, token string) *Client {
	return NewWithHub(baseURL, "", token)
}

// NewWithHub is New plus the hub listener's URL (models and datasets), e.g.
// https://hub.example.com.
func NewWithHub(baseURL, hubURL, token string) *Client {
	c := &Client{
		baseURL: strings.TrimRight(baseURL, "/"),
		hubURL:  strings.TrimRight(hubURL, "/"),
		token:   token,
		http:    &http.Client{Timeout: 30 * time.Second},
	}
	c.CodeRepos = &CodeRepos{c}
	c.ModelRepos = &HubRepos{c: c, kind: "model", plural: "models"}
	c.DataRepos = &HubRepos{c: c, kind: "dataset", plural: "datasets"}
	return c
}

// Request makes one HTTP round trip. body (sent as JSON) and out (decoded from
// the answer) may each be nil; any non-2xx status is an *ApiError.
func (c *Client) Request(method, path string, query url.Values, body, out any) error {
	return c.send(c.baseURL, method, path, query, body, out)
}

// HubRequest is the same round trip against the hub listener.
func (c *Client) HubRequest(method, path string, query url.Values, body, out any) error {
	if c.hubURL == "" {
		return fmt.Errorf("this client has no hub URL; use NewWithHub to reach models and datasets")
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

// RepoOpts carries what a new repo may choose: Create and Fork take them beside
// the "owner/name" strings. An empty Visibility means private.
type RepoOpts struct {
	Visibility  string
	Description string
}

type RepoUpdate struct {
	Description *string `json:"description,omitempty"`
	Visibility  string  `json:"visibility,omitempty"`
}

type Branch struct {
	Name    string `json:"name"`
	Oid     string `json:"oid"`
	Default bool   `json:"default"`
}

type Protection struct {
	Pattern  string `json:"pattern"`
	Remove   bool   `json:"remove,omitempty"`
	NoForce  bool   `json:"no_force"`
	NoDelete bool   `json:"no_delete"`
}

type NewPull struct {
	Title string `json:"title"`
	Head  string `json:"head"`
	Base  string `json:"base"`
	// HeadRepo (another "owner/name") makes it a cross-repo pull: Head then
	// names a branch of that fork, proposed onto this repo's Base.
	HeadRepo    string `json:"head_repo,omitempty"`
	Description string `json:"description"`
}

// FileChange is one file's worth of a commit. ContentBase64 carries the bytes
// (absent when Delete is set); a nil Executable keeps the mode the file has.
type FileChange struct {
	Path          string `json:"path"`
	ContentBase64 string `json:"contentBase64,omitempty"`
	Executable    *bool  `json:"executable,omitempty"`
	Delete        bool   `json:"delete,omitempty"`
}

// NewCommit is one commit of file changes; a non-empty Expect refuses the
// commit if the branch moved, and NewBranch lands it on a new branch instead.
type NewCommit struct {
	Branch    string       `json:"branch"`
	Message   string       `json:"message"`
	Changes   []FileChange `json:"changes"`
	Expect    string       `json:"expect,omitempty"`
	NewBranch string       `json:"newBranch,omitempty"`
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

// newRepoBody is the create/fork wire shape: the destination split back into
// the two fields the server reads.
func newRepoBody(repo string, opts RepoOpts) (map[string]string, error) {
	owner, name, err := splitRepo(repo)
	if err != nil {
		return nil, err
	}
	visibility := opts.Visibility
	if visibility == "" {
		visibility = "private"
	}
	return map[string]string{
		"owner": owner, "name": name,
		"visibility": visibility, "description": opts.Description,
	}, nil
}

func (r *CodeRepos) Create(repo string, opts RepoOpts) (*Repo, error) {
	body, err := newRepoBody(repo, opts)
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.Request("POST", "/v1/repos", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *CodeRepos) List(owner string) ([]Repo, error) {
	var out []Repo
	err := r.c.Request("GET", "/v1/repos", url.Values{"owner": {owner}}, nil, &out)
	return out, err
}

func (r *CodeRepos) Get(repo string) (*Repo, error) {
	p, err := repoPath(repo, "")
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.Request("GET", p, nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *CodeRepos) Update(repo string, change RepoUpdate) error {
	p, err := repoPath(repo, "")
	if err != nil {
		return err
	}
	return r.c.Request("PATCH", p, nil, change, nil)
}

func (r *CodeRepos) Delete(repo string) error {
	p, err := repoPath(repo, "")
	if err != nil {
		return err
	}
	return r.c.Request("DELETE", p, nil, nil, nil)
}

// Fork makes a new repo dst copied from src: objects and refs, never its pull
// requests or settings. Private by default, whatever the source was.
func (r *CodeRepos) Fork(src, dst string, opts RepoOpts) (*Repo, error) {
	p, err := repoPath(src, "/fork")
	if err != nil {
		return nil, err
	}
	body, err := newRepoBody(dst, opts)
	if err != nil {
		return nil, err
	}
	var out Repo
	if err := r.c.Request("POST", p, nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ImportGit makes a new repo whose contents are fetched from a PUBLIC git
// remote — git is a strict "https://" URL (GitHub is one host among any). The
// server answers 202: the repo exists but the fetch runs in the background;
// poll ImportStatus.
func (r *CodeRepos) ImportGit(repo, git string, opts RepoOpts) (*Repo, error) {
	body, err := newRepoBody(repo, opts)
	if err != nil {
		return nil, err
	}
	body["git"] = git
	var out Repo
	if err := r.c.Request("POST", "/v1/repos/import", nil, body, &out); err != nil {
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
	err = r.c.Request("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) Branches(repo string) ([]Branch, error) {
	p, err := repoPath(repo, "/branches")
	if err != nil {
		return nil, err
	}
	var out []Branch
	err = r.c.Request("GET", p, nil, nil, &out)
	return out, err
}

// DeleteBranch takes the head oid the caller saw; the delete is refused if the
// branch moved.
func (r *CodeRepos) DeleteBranch(repo, branch, oid string) error {
	p, err := repoPath(repo, "/branches/"+seg(branch))
	if err != nil {
		return err
	}
	return r.c.Request("DELETE", p, url.Values{"oid": {oid}}, nil, nil)
}

func (r *CodeRepos) Protection(repo string) (any, error) {
	p, err := repoPath(repo, "/protection")
	if err != nil {
		return nil, err
	}
	var out any
	err = r.c.Request("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) Protect(repo string, rule Protection) error {
	p, err := repoPath(repo, "/protection")
	if err != nil {
		return err
	}
	return r.c.Request("POST", p, nil, rule, nil)
}

func (r *CodeRepos) Pulls(repo string) ([]map[string]any, error) {
	p, err := repoPath(repo, "/pulls")
	if err != nil {
		return nil, err
	}
	var out []map[string]any
	err = r.c.Request("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) Pull(repo string, number int64) (map[string]any, error) {
	p, err := repoPath(repo, fmt.Sprintf("/pulls/%d", number))
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.Request("GET", p, nil, nil, &out)
	return out, err
}

func (r *CodeRepos) OpenPull(repo string, body NewPull) (map[string]any, error) {
	p, err := repoPath(repo, "/pulls")
	if err != nil {
		return nil, err
	}
	if body.HeadRepo != "" {
		if _, _, err := splitRepo(body.HeadRepo); err != nil {
			return nil, err
		}
	}
	var out map[string]any
	err = r.c.Request("POST", p, nil, body, &out)
	return out, err
}

func (r *CodeRepos) Comment(repo string, number int64, text string) error {
	p, err := repoPath(repo, fmt.Sprintf("/pulls/%d/comments", number))
	if err != nil {
		return err
	}
	return r.c.Request("POST", p, nil, map[string]string{"body": text}, nil)
}

// Merge asks for the merge as a job (the server answers 202); strategy is
// fast-forward, squash, merge or rebase.
func (r *CodeRepos) Merge(repo string, number int64, strategy string) error {
	p, err := repoPath(repo, fmt.Sprintf("/pulls/%d/merge", number))
	if err != nil {
		return err
	}
	return r.c.Request("POST", p, url.Values{"strategy": {strategy}}, nil, nil)
}

func (r *CodeRepos) Close(repo string, number int64) error {
	p, err := repoPath(repo, fmt.Sprintf("/pulls/%d/close", number))
	if err != nil {
		return err
	}
	return r.c.Request("POST", p, nil, nil, nil)
}

func (r *CodeRepos) Compare(repo, base, head string) (map[string]any, error) {
	p, err := repoPath(repo, "/compare")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.Request("GET", p, url.Values{"base": {base}, "head": {head}}, nil, &out)
	return out, err
}

func (r *CodeRepos) Commit(repo string, body NewCommit) (map[string]any, error) {
	p, err := repoPath(repo, "/commits")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.Request("POST", p, nil, body, &out)
	return out, err
}

func (r *CodeRepos) Signature(repo, sha string) (map[string]any, error) {
	p, err := repoPath(repo, "/commits/"+seg(sha)+"/signature")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = r.c.Request("GET", p, nil, nil, &out)
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
func (h *HubRepos) Create(repo string, private bool) (map[string]any, error) {
	body, err := h.body(repo, map[string]any{"private": private})
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.HubRequest("POST", "/api/repos/create", nil, body, &out)
	return out, err
}

func (h *HubRepos) Get(repo string) (map[string]any, error) {
	p, err := h.path(repo, "")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.HubRequest("GET", p, nil, nil, &out)
	return out, err
}

// Refs answers branches and tags, as the hub spells them.
func (h *HubRepos) Refs(repo string) (map[string]any, error) {
	p, err := h.path(repo, "/refs")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.HubRequest("GET", p, nil, nil, &out)
	return out, err
}

func (h *HubRepos) SetPrivate(repo string, private bool) error {
	p, err := h.path(repo, "/settings")
	if err != nil {
		return err
	}
	return h.c.HubRequest("PUT", p, nil, map[string]any{"private": private}, nil)
}

func (h *HubRepos) Delete(repo string) error {
	body, err := h.body(repo, nil)
	if err != nil {
		return err
	}
	return h.c.HubRequest("DELETE", "/api/repos/delete", nil, body, nil)
}

// ImportGit makes a new repo whose contents — large files included, through
// git-lfs — are fetched from a PUBLIC git remote; git is a strict "https://"
// URL (the Hugging Face Hub is one host among any). The server answers 202;
// poll ImportStatus.
func (h *HubRepos) ImportGit(repo, git string, private bool) (map[string]any, error) {
	p, err := h.path(repo, "/import")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.HubRequest("POST", p, nil, map[string]any{"from": git, "private": private}, &out)
	return out, err
}

// ImportStatus answers {from, state: queued|running|done|failed, detail?}.
func (h *HubRepos) ImportStatus(repo string) (map[string]any, error) {
	p, err := h.path(repo, "/import")
	if err != nil {
		return nil, err
	}
	var out map[string]any
	err = h.c.HubRequest("GET", p, nil, nil, &out)
	return out, err
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
