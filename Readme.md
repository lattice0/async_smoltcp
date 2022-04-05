# Hyper VPN with OpenVPN3 in userspace mode (no tun/tap interface)

An example of merging the hyper http library with https://github.com/lattice0/true_libopenvpn3_rust and https://github.com/lattice0/async_smoltcp

Basically, it's Hyper HTTP lib with SmolTCP TCP/IP stack plugged into OpenVPN3 (C++) code, so you can send/receive HTTP packets in userspace without ever interacting with the tun/tap interface of the operating system. With this, you can open multiple OpenVPN connections on Android, for example, where it wouldn't be possible without root. 

# Cloning

Don't forget to clone recursively

```bash
git clone --recurse-submodules -j8 https://github.com/lattice0/hyper_vpn
```

# Contributing

Everything is in a draft state so it would be nice to have contributions to organize things
