<img src="https://raw.githubusercontent.com/schell/teleform/main/globe.png" alt="teleform logo" width="250">

# Teleform

## What is Teleform?

Teleform is an Infrastructure-as-Code (IaC) library for Rust, offering a
flexible and powerful alternative to tools like Terraform and Pulumi. It allows
developers to describe infrastructure changes as a Directed Acyclic Graph (DAG),
providing direct interaction with platform APIs without additional abstraction layers.

## Why use Teleform?

- **Flexibility**: Leverage the full power of Rust to define and manage your
  infrastructure.
- **Direct API Interaction**: No wrappers over platform-specific resources,
  allowing for precise and domain-specific configurations.
- **Version Control**: Infrastructure definitions are Rust code, easily tracked
  and managed with version control systems.

## How does it work?

### High-Level Overview

Teleform operates on the concept of local and remote states of resources, using
these states to determine necessary actions such as creating, updating, or
deleting resources.

### Resources

Resources are defined as structs implementing the `Resource` trait, with methods
for `create`, `read`, `update`, and `delete`. These methods are explicitly
`unimplemented!` for developer convenience, allowing you to define only the
methods you need immediately.

### Providers

Providers are associated types on the `Resource` trait, facilitating interaction
with the platform's API. For example, AWS uses `aws_config::SdkConfig` as its provider.

### Store

The store manages the synchronization and serialization of your resources to the
filesystem.
It is the main structure you interact with when defining your infrastructure in
a command-line program.

## Target audience

Teleform is ideal for developers, especially those in solo or small team
environments, seeking a more general and flexible solution to IaC. It is also
suitable for those looking to migrate away from Terraform.

## WARNING: Alpha Software!

This software is in its early stages and primarily works along a happy path. Use
it with caution and contribute if you can!
