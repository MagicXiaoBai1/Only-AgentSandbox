.PHONY: build test check clean

build:
	go build -trimpath -o bin/oas-vm-net ./cmd/oas-vm-net

test:
	go test ./...

check:
	go test -race ./...
	go vet ./...

clean:
	rm -rf bin