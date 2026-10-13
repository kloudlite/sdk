// Package kloudlite is the Kloudlite SDK for Go — stdlib only.
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client:
//
//	kl := kloudlite.New("https://api.example.com", "…")
//	repo, err := kl.Repos.Create(kloudlite.NewRepo{Owner: "acme", Name: "payments"})
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
	Repos *Repos

	baseURL string
	token   string
	http    *http.Client
}

// New makes a client. token may be empty for anonymous access to public data.
func New(baseURL, token string) *Client {
	c := &Client{
		baseURL: strings.TrimRight(baseURL, "/"),
		token:   token,
		http:    &http.Client{Timeout: 30 * time.Second},
	}
	c.Repos = &Repos{c}
	return c
}

// Request makes one HTTP round trip. body (sent as JSON) and out (decoded from
// the answer) may each be nil; any non-2xx status is an *ApiError.
func (c *Client) Request(method, path string, query url.Values, body, out any) error {
	u := c.baseURL + path
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

// Repos is the code-repository module: /v1/repos.
type Repos struct{ c *Client }

type Repo struct {
	ID          string `json:"_id"`
	Owner       string `json:"owner"`
	Name        string `json:"name"`
	Public      bool   `json:"public"`
	Description string `json:"description"`
	CreatedBy   string `json:"created_by"`
	CreatedAt   int64  `json:"created_at"`
}

type NewRepo struct {
	Owner string `json:"owner"`
	Name  string `json:"name"`
	// Empty means private.
	Visibility  string `json:"visibility,omitempty"`
	Description string `json:"description"`
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
	// HeadRepo ("owner/name") makes it a cross-repo pull: Head then names that fork's branch.
	HeadRepo    string `json:"head_repo,omitempty"`
	Description string `json:"description"`
}

// ForkRepo names a fork's destination; the source goes in Fork's arguments.
type ForkRepo struct {
	Owner string `json:"owner"`
	Name  string `json:"name"`
	// Empty means private, whatever the source was.
	Visibility  string `json:"visibility,omitempty"`
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

func repoPath(owner, name, tail string) string {
	return "/v1/repos/" + seg(owner) + "/" + seg(name) + tail
}

func (r *Repos) Create(body NewRepo) (*Repo, error) {
	var out Repo
	if err := r.c.Request("POST", "/v1/repos", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *Repos) List(owner string) ([]Repo, error) {
	var out []Repo
	err := r.c.Request("GET", "/v1/repos", url.Values{"owner": {owner}}, nil, &out)
	return out, err
}

func (r *Repos) Get(owner, name string) (*Repo, error) {
	var out Repo
	if err := r.c.Request("GET", repoPath(owner, name, ""), nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *Repos) Update(owner, name string, change RepoUpdate) error {
	return r.c.Request("PATCH", repoPath(owner, name, ""), nil, change, nil)
}

func (r *Repos) Delete(owner, name string) error {
	return r.c.Request("DELETE", repoPath(owner, name, ""), nil, nil, nil)
}

// Fork makes a new repo copied from src: objects and refs, never its pull requests or settings.
func (r *Repos) Fork(srcOwner, srcName string, dst ForkRepo) (*Repo, error) {
	var out Repo
	if err := r.c.Request("POST", repoPath(srcOwner, srcName, "/fork"), nil, dst, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (r *Repos) Branches(owner, name string) ([]Branch, error) {
	var out []Branch
	err := r.c.Request("GET", repoPath(owner, name, "/branches"), nil, nil, &out)
	return out, err
}

// DeleteBranch takes the head oid the caller saw; the delete is refused if the
// branch moved.
func (r *Repos) DeleteBranch(owner, name, branch, oid string) error {
	return r.c.Request("DELETE", repoPath(owner, name, "/branches/"+seg(branch)),
		url.Values{"oid": {oid}}, nil, nil)
}

func (r *Repos) Protection(owner, name string) (any, error) {
	var out any
	err := r.c.Request("GET", repoPath(owner, name, "/protection"), nil, nil, &out)
	return out, err
}

func (r *Repos) Protect(owner, name string, rule Protection) error {
	return r.c.Request("POST", repoPath(owner, name, "/protection"), nil, rule, nil)
}

func (r *Repos) Pulls(owner, name string) ([]map[string]any, error) {
	var out []map[string]any
	err := r.c.Request("GET", repoPath(owner, name, "/pulls"), nil, nil, &out)
	return out, err
}

func (r *Repos) Pull(owner, name string, number int64) (map[string]any, error) {
	var out map[string]any
	err := r.c.Request("GET", repoPath(owner, name, fmt.Sprintf("/pulls/%d", number)), nil, nil, &out)
	return out, err
}

func (r *Repos) OpenPull(owner, name string, body NewPull) (map[string]any, error) {
	var out map[string]any
	err := r.c.Request("POST", repoPath(owner, name, "/pulls"), nil, body, &out)
	return out, err
}

func (r *Repos) Comment(owner, name string, number int64, text string) error {
	return r.c.Request("POST", repoPath(owner, name, fmt.Sprintf("/pulls/%d/comments", number)),
		nil, map[string]string{"body": text}, nil)
}

// Merge asks for the merge as a job (the server answers 202); strategy is
// fast-forward, squash, merge or rebase.
func (r *Repos) Merge(owner, name string, number int64, strategy string) error {
	return r.c.Request("POST", repoPath(owner, name, fmt.Sprintf("/pulls/%d/merge", number)),
		url.Values{"strategy": {strategy}}, nil, nil)
}

func (r *Repos) Close(owner, name string, number int64) error {
	return r.c.Request("POST", repoPath(owner, name, fmt.Sprintf("/pulls/%d/close", number)), nil, nil, nil)
}

func (r *Repos) Compare(owner, name, base, head string) (map[string]any, error) {
	var out map[string]any
	err := r.c.Request("GET", repoPath(owner, name, "/compare"),
		url.Values{"base": {base}, "head": {head}}, nil, &out)
	return out, err
}

func (r *Repos) Commit(owner, name string, body NewCommit) (map[string]any, error) {
	var out map[string]any
	err := r.c.Request("POST", repoPath(owner, name, "/commits"), nil, body, &out)
	return out, err
}

func (r *Repos) Signature(owner, name, sha string) (map[string]any, error) {
	var out map[string]any
	err := r.c.Request("GET", repoPath(owner, name, "/commits/"+seg(sha)+"/signature"), nil, nil, &out)
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
