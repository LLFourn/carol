# Carol - A programmable third party for any protocol

Carol is a [*serverless*](https://en.wikipedia.org/wiki/Serverless_computing) computing service that
runs user uploaded programs. At the moment the programs run on only one machine but in the end goal
is to have the programs be run between *federations* of carol nodes. The purpose of Carol is not to
"outsource" computing to the cloud like in traditional compute providers. A user asks Carol to run
their program so that others can have confidence that the program will be run faithfully.

## Quick start

First, make sure you've got the WASM target installed:

```
rustup target add wasm32-unknown-unknown
```

### Install `carlo` (optional)

```sh
cargo install --path crates/carlo
```

Otherwise you have to run `cargo run -p carlo` form the project directory instead of running `carlo`.

You should be able to create a machine on an existing carol instance like `carol.computer`:

```sh
cd example-guests/bitmex_oracle
carlo create --carol-url https://carol.computer
```


### Run a machine locally

There are example guest machine definitions in [`example-guests`](./example-guests). Let's copy one
of them and run it on a temporary machine:

``` shell
cd .. # move above project directory
cp -r carol/example-guests/bitmex_oracle my_machine
cd my_machine
carlo run
```

if you're using `cargo` from the project directory, use: `cargo run -p carlo -- run -p ../my_machine` from the project directory.

You should see a few urls in the output for the machine. Try visiting the machine's HTTP url to see
it's HTML landing page.

### Run the machine on a public carol server

After developing a machine we want to deploy we can run it on a public carol server:


``` shell
carlo create --carol-url https://carol.computer
```

This will output a url to the machine you just created.
## Run your own carol node

### Install

First clone the repository and then `cd` into it and run:

``` sh
cargo install --path crates/carol
carol --help
```

..or it can be run from the project directory:

``` sh
cargo run -p carol -- --help
```

### Configure

Then generate a config file.

``` sh
carol --cfg carol.yml config-gen
```

### Run

This will generate a default configuration (along with some secret keys!) and put them in `carol.yml`.

``` sh
carol --cfg carol.yml run &
```

## Full carlo workflow

To compile a standalone WASM binary. Here we just compile one of the examples in `example-guests`
(must be run in project directory).

``` sh
wasm_output_file=$( carlo build -p bitmex_oracle )
```

Then upload it to carol:

``` sh
carol_url=http://localhost:8000
binary_id=$( carlo -q upload --carol-url "${carol_url}" --binary "${wasm_output_file}" )
```

Note this `id` is just a hash of the binary. In general it's intended for client software to
generate it locally and check it against what's returned rather than just blindly trusted the carol
server.

### Create the machine

Carol machines are created from a binary and a parameterization array. Most machines will have an empty parameterization for now so we make an empty POST request to t

``` sh
machine_id=$( carlo -q create --carol-url "${carol_url}" --binary-id "${binary_id}" )
```

Note this `id` is a hash of the binary and the (empty) parameterization vector. In general it's
intended for client software to generate it locally and check it against what's returned rather than
just blindly trusted the carol server.

### Activate the machine

The machine we've created has a few ways of activating it as defined [the code](./example-guests/bitmex/src/lib.rs ) we compiled to the binary.
At the time of writing it's:

- `attest_to_price_at_minute`
- `bit_decompose_attest_to_price_at_minute`

We can actual inspect the binary for its activation methods with:

``` sh
carlo api -p bitmex_oracle
```

(this just displays the names of them for now)

Let's send a HTTP request to the machine which will activate the `attest_to_price_at_minute` which will fetch the price of the symbol we ask for at the time we ask for.


```sh
curl -vG "${carol_url}/machines/${machine_id}/http/attest_to_price_at_minute" \
-d time=2023-04-16T12:30:00Z \
-d symbol=.BXBT
```

which at the time of writing returns:

``` json
{
  "Ok": {
    "price": 30264,
    "signature": "8d737860c57c0463ab532127359c0a7fbc9fa1bf56b120ad3b724637fb3a3c08d621ce5afe20de25889d14c7e23a0a4a19961cc08596f2c82fd84b9b00fa24b5fc4e67226300d855f6e51176d7ef73525e37d7baad6dae701271a0ede593000d"
  }
}
```

## Roadmap

### 1. Stateless oracles

The first milestone is to have carol functioning as a programmable BLS-based DLC oracle for the protocol described in *[Cryptographic Oracle-Based Conditional Payments]*.
To do this it doesn't need state and it doesn't need to communicate with other carol nodes.

### 2. TODO Local state

At some point we want to allow programs to store state that they can access when they are re-activated as well as generate secret keys for `secp256k1`, `bls12_381`

### 3. TODO Federated state

Finally, we want programs to be able to store state on multiple carol nodes working as a federation. Secret keys would be Shamir secret shared across these nodes as well.



[Cryptographic Oracle-Based Conditional Payments]: https://eprint.iacr.org/2022/499
