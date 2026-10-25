package kloudlite

import "fmt"

// Secrets is an owner's named secrets, over /v1/secrets. A value goes in and
// never comes back: List answers names with who and when. Reference one from
// a workspace env as "${{ secrets.NAME }}".
type Secrets struct{ c *Client }

// SecretOptions picks whose secret: Scope "mine" (the default) is your own in
// the team owner (for your own handle, your personal one); "team" is the team's
// shared secret and needs an admin.
type SecretOptions struct {
	Scope string
}

func secretPath(owner, name string, opts []SecretOptions) (string, error) {
	scope := "mine"
	if len(opts) > 0 && opts[0].Scope != "" {
		scope = opts[0].Scope
	}
	if scope != "mine" && scope != "team" {
		return "", fmt.Errorf("scope is 'mine' or 'team', got %q", scope)
	}
	p := "/v1/secrets/" + seg(owner)
	if scope == "mine" {
		p += "/mine"
	}
	if name != "" {
		p += "/" + seg(name)
	}
	return p, nil
}

func (s *Secrets) Set(owner, name, value string, opts ...SecretOptions) error {
	p, err := secretPath(owner, name, opts)
	if err != nil {
		return err
	}
	return s.c.call("PUT", p, nil, map[string]string{"value": value}, nil)
}

func (s *Secrets) List(owner string, opts ...SecretOptions) ([]map[string]any, error) {
	p, err := secretPath(owner, "", opts)
	if err != nil {
		return nil, err
	}
	var out []map[string]any
	err = s.c.call("GET", p, nil, nil, &out)
	return out, err
}

// Delete is a 409 while a workspace or environment still references the
// secret; the error text names them.
func (s *Secrets) Delete(owner, name string, opts ...SecretOptions) error {
	p, err := secretPath(owner, name, opts)
	if err != nil {
		return err
	}
	return s.c.call("DELETE", p, nil, nil, nil)
}
