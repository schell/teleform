<img src="https://raw.githubusercontent.com/schell/teleform/main/globe.png" alt="teleform logo" width="250" align="right">

# teleform

[See the example](crates/example/src/main.rs).

## what

Infrastructure-as-code like `terraform`, but in a Rust library. This makes
it easy to integrate infrastructure setup and teardown into your project's
xtask.

### building and running the example

First build the example lambda using [cargo lambda](https://www.cargo-lambda.info/):
```
cargo lambda build --release --arm64 --output-format zip
```

Then run the example program with your AWS account id:
```
cargo run -p example -vvv --account-id xxxxxxxxxxxx
```

When you're ready to apply the changes (building all infrastructure):
```
cargo run -p example -vvv --account-id xxxxxxxxxxxx --apply
```

That should print out a url where you can play with your stack.

When you're ready to tear it all down run:
```
cargo run -p example -vvv --account-id xxxxxxxxxxxx --apply --delete
```

## why

IaC is a good idea. It's good to have options. Rust is great, and using a
full-featured language provides a lot of flexibility. Also some people
have a chip on their shoulder about `terraform`.

## how

### high level idea

The trait `TeleSync` allows you to write `create`, `update`, and `delete`
implementations for resources. It also specifies how resources are
`composite`d from the IaC definition (aka your code) and the store file, as well
as what fields cause recreations (`should_recreate`) and updates
(`should_update`). The `teleform-derive` crate provides a derive macro and
attributes that make implementing this trait pretty easy for new resources.

### resources

A resource is a struct with fields of either type `Local<T>` or `Remote<T>`,
which are wrappers for local values and remote values, respectively. A
local value is a value that is known before resource creation - like the
name of an AWS Lambda function. A remote value is one that can only be
known _after_ creation - like the ARN of the same Lambda function.

Each resource must implement `TeleSync`.

### providers

A provider is an associated type on `TeleSync` that helps sync your resources to
your IaC definition. For AWS, the provider is `SdkConfig`, which is used to
create a client for each AWS sub-service.

#### included providers

There are currently only stub providers offering a very limited number of
resources. Part of the motivation to opensource this project is to vet the idea
and see if folks are willing to contribute some resources.

See the `tele::aws` module for included resources.

### store

The store is a `BTreeMap` of resources that get syncronized and serialized. It is
the main structure you interact with in your command line program when defining
your infrastructure.

## alpha

This software is super-alpha! It pretty much works, but I wouldn't base your corp
on it, unless you're a computer cowboy like me. Yeehaw!
