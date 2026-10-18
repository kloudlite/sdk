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

	kloudlite "github.com/kloudlite/sdk/go"
)

func main() {
	kl := kloudlite.New(kloudlite.Options{
		BaseURL: os.Getenv("KLOUDLITE_URL"),
		Token:   os.Getenv("KLOUDLITE_TOKEN"),
	})

	upstream := "acme/payments" // the repo being contributed to
	myFork := "dev/payments"    // where the fork lands (a handle or a team slug)

	// 1. Fork: objects and refs, never the upstream's pull requests or settings.
	fork, err := kl.CodeRepos.Fork(kloudlite.ForkRepo{Repo: upstream, To: myFork})
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("forked into %s/%s\n", fork.Owner, fork.Name)

	// 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
	content := base64.StdEncoding.EncodeToString([]byte("# Payments\n\nFixed rounding.\n"))
	out, err := kl.CodeRepos.Commit(kloudlite.CommitChanges{
		Repo:      myFork,
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
	pr, err := kl.CodeRepos.OpenPull(kloudlite.OpenPull{
		Repo:     upstream,
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
	if _, err := kl.CodeRepos.Compare(kloudlite.CompareRefs{
		Repo: upstream,
		Base: "main",
		Head: fmt.Sprintf("pull/%d/head", number),
	}); err != nil {
		log.Fatal(err)
	}
	_ = kl.CodeRepos.Comment(kloudlite.CommentOnPull{Repo: upstream, Number: number, Body: "Ready for review"})

	// 5. Merge. 202 — a job; the pull reports where it got to.
	if err := kl.CodeRepos.Merge(kloudlite.MergePull{Repo: upstream, Number: number, Strategy: "squash"}); err != nil {
		log.Fatal(err)
	}
	for i := 0; i < 30; i++ {
		got, err := kl.CodeRepos.Pull(kloudlite.GetPull{Repo: upstream, Number: number})
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
