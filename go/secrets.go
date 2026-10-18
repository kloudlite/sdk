package kloudlite

import "net/url"

// Secrets is an owner's named secrets, over /v1/secrets. A value goes in and
// never comes back: List answers names with who and when. Reference one from
// a workspace env as "${{ secrets.NAME }}".
type Secrets struct{ c *Client }

func (s *Secrets) Set(owner, name, value string) error {
	return s.c.call("PUT", "/v1/secrets/"+seg(owner)+"/"+seg(name), nil, map[string]string{"value": value}, nil)
}

func (s *Secrets) List(owner string) ([]map[string]any, error) {
	var out []map[string]any
	err := s.c.call("GET", "/v1/secrets/"+seg(owner), url.Values{}, nil, &out)
	return out, err
}

// Delete is a 409 while a workspace or environment still references the
// secret; the error text names them.
func (s *Secrets) Delete(owner, name string) error {
	return s.c.call("DELETE", "/v1/secrets/"+seg(owner)+"/"+seg(name), nil, nil, nil)
}
