# DEVLOG

## PINNED - Pro / Con of using AWS' crates instead of Terraform

| Pro          | Con                |
|--------------|--------------------|
| Rust is a language | |
| Rust is typechecked | |
| crates.io docs format | |
| serialized infrastructure data can easily be used in the application | |
| | Lots of examples, lots of SO entries |
| | Of course, you don't do CRUD by hand |
| I can look at the CRUD code | |
| I can "infrastructure" anything! Like content. | |
| Using a DSL that is shrink-wrapped to my application. | |
| | Any needed thing your current DSL doesn't cover must be written by you. |
| | How to accept possible infrastructure changes from unpriviledged devs? |
| | You have to wait for compile times to sync infrastructure |

## Sun 5 Oct

V2 is nearly ready.
It's based on `dagga`, so now CRUD operations are sequenced as a DAG.
This allows the DAG to be inspected before it's executed.

I'm currently wondering if maybe it would be better to ditch having a saved
"remote" value and instead _always load the value from the platform_?

## Mon Sep 29 2025

I'm attempting a rework that models the problem a little tighter.

* defining
  - I want to define in code, and pair with a local cache of a previous syncing
* diffing
  - I want to generate a diff that can be used to sync, with an inherent order
* syncing

I ended up just splitting out the `TeleSync` trait into two traits, `TeleSync` and `TeleCmp`.

It would be nice to do the following in the future:

1. Be able to define the entire stack of infrastructure in one top-level struct.
2. Diff the in-memory definition of the struct against a cached struct on the filesystem,
   this returns a DAG.
3. Inspect the DAG. Store the DAG?. Execute the DAG. This returns a new struct.
4. Persist the new struct to the filesystem.
5. Allows import?
6. Idempotent apply.

## Thu Oct 19 2023

It was the ARN for the "add_permissions" call! You have to specify the *version*
of the lambda that your HTTP API gateway can call 🤦.

So now that I have that figured out I feel like the `teleform` crate is a success.
I'm firmly in the "pain of working with the platform" phase.

## Wed Oct 18 2023

### AWS is madness - permissions and such

I think I have a pretty workable system built up, and now the hard part is
dealing with AWS's idiosyncracies - of which there are *many*! I've been
having a hell of a time invoking my lambda from my http apigateway but I
just found a [doc that might
help](https://docs.aws.amazon.com/lambda/latest/dg/services-apigateway.html).

* without a default stage I get a 400 from my route
* with a default stage:
  * I get a 500 from my route /bears
  * I get a 400 from anything else (which makes sense, actually)

## Mon Oct 16 2023

### Default resources

Sometimes after creating a resource (like an ApiGatewayV2 api), the platform
creates some knock-on resources (like default integrations and a default route).
What is a good way to integrate these into the store?

I think maybe making them part of the struct is the easiest way to go.

#### Overthinking it

Turns out I was overthinking it. The default resources were being created because
of the parameters I was passing to the `create_api` function. So I dodged the
need to fix this problem and am kicking the can down the road.

## Mon Oct 16 2023

### Deletion ordering problem

When deleting resources, some have to be done in order. I have to think of some
way to order the deletions to support this.

I think a cheap way is to call the `prune` functions in a certain order.

## Mon Oct 16 2023

### Remote values problem

Using `Remote` as a value that gets determined _after_ creation has worked so far.
I created the type because you often have to figure out if a value has changed,
but when writing your infrastructure declaration using `Store::sync`, I don't
know that value because it hasn't been created. After creation though, I do know,
but I don't want to have to change the value in the code explicitly, so
`Remote`'s `PartialEq::partial_eq` will return `true` when anything is compared
against `Unknown`. This solved the problem where the store would think a resource
has to be updated because an ARN is determined after creation.

The problem it doesn't solve is when _another value causes an update_. Then the
value of `Remote::Unknown` as its used in the callsite gets written to the store
file, overwriting the previously known value (that was determined after creation).

### The fix

I think the first step would be to combine the callsite resource struct
with the stored resource struct field-by-field to get a composite callsite struct,
and then compare that with the stored struct. Each field would have to be a type
that implements a new trait (it would have a default implementation) that would
compare the two and return one. `Remote`'s impl of this trait could always return
the known value (if either value is known). Then it could have a derived `PartialEq`.

### Remote AND Local values

I fixed the issue by having `Remote` and `Local` values. `Local` values are
values that we know before _any_ resource creation. `Remote` values are values
that are known after only resource creation. This includes downstream remote
values - values that are input to other resources but are not determined by that
resource's creation.

I ended up only having to write two impls since each field of a resource should
be wrapped in `Local` or `Remote`.

## Fri Oct 13 2023

Proc-macros to the rescue! `TeleSync` now has a derive macro w/ some much needed
attributes that help make writing `TeleSync` impls pretty easy.

## Wed Oct 11 2023

Writing infrastructure like this is actually pretty easy, but it becomes repetitive
and painful writing TeleSync impls for each type. Of course if this were to become
a library on its own that stuff would already be written and then this would be
pretty nice, but I don't want to be responsible for maintaining every possible
infrastructure type and their TeleSync impls.

## Sun Oct 8 2023

I've been WAY overthinking this. We don't need diffs, really. We also don't need
DAGs. Since the infrastructure is declared in Rust, the language itself ensures
ids from one resource exist before using them in another resource. In the end
what we need is a trait that does crud for the backend (AWS, DO, etc) and tells
us to updated or recreate given the current state of a resource and its previous
state. The store file then becomes a simple list/map of resources.

## Fri Oct 6 2023

It's my 40th birthday! I have been programming for 21 years :)

## Thu Oct 5 2023

So far, doing this (IaC) all by hand is pretty painful, but getting better as I
flesh things out.
I have a struct that contains my infra declaration (at this point just a couple
of AWS IAM policies in a hashmap) and I'm using diff-struct to compare that
struct to the last version that was serialized to disk as JSON.
I then walk the changes and create / update if needed.
Delete is different though, because you need to know if any removed ARNs are
still used. I think I'm going to handle that with a special id.

### A special ID

There are three ID/ARN scenarios we have to support:

1. A resource doesn't yet have an ID, and its ID might be referenced by another
   resource.
2. A resource has an ID and is being serialized.
3. A resource has an ID and is being deserialized. It must correspond to other
   resources with a matching ID.

### Making and applying a diff

* Does it need to be created, updated or deleted?
* How to execute creation
* What data does it need to conduct an update?
* How to execute an update?
* How to execute a delete?

## Wed Oct 4 2023

### AWS stuff!

Starting up this project today and have realized that Rust's AWS offerings have
_really_ improved over the last two years since I last wrote a lambda (while at
takt.io / formation.io).

[`awslabs`](https://crates.io/teams/github:awslabs:rust-sdk-owners) have come
out with a whole slew of SDKs that make me feel like maybe I could ditch
terraform and just abuse those raw SDKs to build infrastructure. Maybe that's a
big mistake - we'll find out!

### Getting to work

For setting up the infra I'm following the tutorial
[here](https://docs.aws.amazon.com/lambda/latest/dg/services-apigateway-tutorial.html).
