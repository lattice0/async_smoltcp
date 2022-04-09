# Async smoltcp

In development. Implements tokio's `AsyncWrite` and `AsyndRead` for smoltcp, and wraps everything in a struct with support for tun, tap and virtual tun. 

Currently you'll possibly want to use like this

`async_smoltcp = {git = "https://github.com/lattice0/async_smoltcp", features=["default", "log"]}`

# TODO: 

- bump smoltcp version 
