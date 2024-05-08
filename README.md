# php-downloader

A simple utility to download and manage one or more complete PHP build trees.  This is mostly a personal project that is useful given the way I like to develop PHP extensions (within the entire PHP build tree).

## Features

- **Download PHP Sources**: Download either a specific version or the latest patch of a given major/minor. 
- **Custom Configuration**: Extract and build the source trees using shell script hooks.
- **Version Management**: Upgrade a given php-MAJOR.MINOR.PATCH build tree to the latest version and deleting the old tree.

### Installation

Clone the php-downloader repository to your local machine using:

```bash
git clone https://github.com/michael-grunder/php-downloader.git
cd php-downloader && cargo build
