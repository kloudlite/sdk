// The fork workflow, end to end: fork a repo, commit onto a new branch of the
// fork, propose it back upstream as a cross-repo pull request, and merge it.
//
//	KLOUDLITE_URL=https://api.example.com KLOUDLITE_TOKEN=… go run ./example
package main

import (
	"encoding/base64"
	"fmt"
	"log"
	"os"
	"time"

	kloudlite "github.com/kloudlite/rustic-git/sdk/go"
)

func main() {
	kl := kloudlite.New(os.Getenv("KLOUDLITE_URL"), os.Getenv("KLOUDLITE_TOKEN"))

	upstream := "acme/payments" // the repo being contributed to
	myFork := "dev/payments"    // where the fork lands (a handle or a team slug)

	// 1. Fork: objects and refs, never the upstream's pull requests or settings.
	fork, err := kl.Repos.Fork(upstream, myFork, kloudlite.RepoOpts{})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("forked into %s/%s\n", fork.Owner, fork.Name)

	// 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
	content := base64.StdEncoding.EncodeToString([]byte("# Payments\n\nFixed rounding.\n"))
	out, err := kl.Repos.Commit(myFork, kloudlite.NewCommit{
		Branch:    "main",
		NewBranch: "fix-rounding",
		Message:   "Fix rounding in invoice totals",
		Changes:   []kloudlite.FileChange{{Path: "README.md", ContentBase64: content}},
	})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("committed", out["commit"], "on", out["branch"])

	// 3. Propose it back: head names the FORK's branch, HeadRepo says where it lives.
	pr, err := kl.Repos.OpenPull(upstream, kloudlite.NewPull{
		Title:    "Fix rounding in invoice totals",
		Head:     "fix-rounding",
		Base:     "main",
		HeadRepo: myFork,
	})
	if err != nil {
		log.Fatal(err)
	}
	number := int64(pr["number"].(float64))
	fmt.Println("opened", pr["_id"])

	// 4. The diff upstream reviews is the mirror: pull/{n}/head.
	if _, err := kl.Repos.Compare(upstream, "main", fmt.Sprintf("pull/%d/head", number)); err != nil {
		log.Fatal(err)
	}
	_ = kl.Repos.Comment(upstream, number, "Ready for review")

	// 5. Merge. 202 — a job; the pull reports where it got to.
	if err := kl.Repos.Merge(upstream, number, "squash"); err != nil {
		log.Fatal(err)
	}
	for i := 0; i < 30; i++ {
		got, err := kl.Repos.Pull(upstream, number)
		if err != nil {
			log.Fatal(err)
		}
		if state, _ := got["state"].(string); state != "open" {
			fmt.Println("pull is", state)
			return
		}
		time.Sleep(2 * time.Second)
	}
}
