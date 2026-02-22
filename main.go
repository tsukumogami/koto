package main

import (
	"fmt"
	"os"

	"github.com/tsukumogami/koto/internal/buildinfo"
)

func main() {
	if len(os.Args) > 1 && os.Args[1] == "version" {
		fmt.Println("koto", buildinfo.Version())
		return
	}

	fmt.Println("koto", buildinfo.Version())
}
