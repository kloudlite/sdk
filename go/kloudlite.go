// Package kloudlite is the Kloudlite SDK for Go — stdlib only.
//
// The client speaks to the browse API with HTTP Basic auth: the username names
// the owner the token belongs to (git's "x" placeholder also works), the token
// is the secret. No credentials is still a valid client — public repositories
// admit anonymous reads.
package kloudlite

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
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
	baseURL  string
	username string
	token    string
	http     *http.Client
}

// New makes a client. token may be empty for anonymous access to public
// repositories; username defaults to git's "x" placeholder when empty.
func New(baseURL, username, token string) *Client {
	if username == "" {
		username = "x"
	}
	return &Client{
		baseURL:  strings.TrimRight(baseURL, "/"),
		username: username,
		token:    token,
		http:     &http.Client{Timeout: 30 * time.Second},
	}
}

// Request makes one HTTP round trip. body (sent as JSON) and out (decoded from
// the answer) may each be nil; any non-2xx status is an *ApiError.
func (c *Client) Request(method, path string, body, out any) error {
	var rdr io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		rdr = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, c.baseURL+path, rdr)
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", "kloudlite-sdk-go/"+Version)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.token != "" {
		req.SetBasicAuth(c.username, c.token)
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
