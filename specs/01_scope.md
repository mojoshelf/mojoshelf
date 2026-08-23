# MojoRegistry

## Intro

The Mojo Registry is a helper tool, website and database to track re-usable Mojo
packages.

Since Mojo packages are not defined yet by Modular the current approach is to
use git submodules.

This code will be deprecated once Modular supports packages.

## Architecture

The code will be deployed at Cloudflare and will be comprised of:

1. A Database defining packages
2. A Website with authentication allowing admin users to add or edit packages and any
   user to download information about packages.
3. A CLI tool allowing users to add packages using git submodules.
