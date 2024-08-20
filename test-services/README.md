# Test services

To build (from the repo root):

```shell
$ podman build -f test-services/Dockerfile -t restatedev/rust-test-services . 
```

To run (download the [sdk-test-suite](https://github.com/restatedev/sdk-test-suite) first):

```shell
$ java -jar restate-sdk-test-suite.jar run restatedev/rust-test-services
```