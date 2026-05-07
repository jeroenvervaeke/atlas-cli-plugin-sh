# atlas-cli-plugin-sh

Atlas CLI plugin that launches [mongosh](https://www.mongodb.com/docs/mongodb-shell/) connected to an Atlas cluster — no manual connection string needed.

## Install

```
atlas plugin install jeroenvervaeke/atlas-cli-plugin-sh
```

## Usage

### Interactive shell

**Usage**
```shell
atlas sh --cluster <cluster-name> [--project-id <project-id>] [--profile <profile>]
```

**Example**
```
❯ atlas sh --cluster MyCluster
Current Mongosh Log ID:    683a1fc72d4e890b12cd4a77
Connecting to:        mongodb+srv://<credentials>@mycluster.rx4kpqz.mongodb.net/?authSource=admin&appName=mongosh+2.7.0
Using MongoDB:        8.0.23
Using Mongosh:        2.7.0

Atlas mycluster-shard-0 [primary] test>
```

### Run a single command

**Usage**
```shell
atlas sh --cluster <cluster-name> --eval "<mongosh expression>"
```

**Example**
```
❯ atlas sh --cluster MyCluster --eval "show dbs"
admin  0 B
local  0 B
```

### Pass mongosh arguments

Any flags not recognized by `atlas sh` are forwarded verbatim to `mongosh`.

```shell
atlas sh --cluster MyCluster --quiet --norc
```

## Requirements

- [Atlas CLI](https://github.com/mongodb/mongodb-atlas-cli) 1.35.0+
- [mongosh](https://www.mongodb.com/try/download/shell) installed and on `PATH`

## How it works

The plugin fetches connection credentials from Atlas via the Atlas CLI, stores them securely in the system keychain, then invokes `mongosh` with the appropriate connection string. On exit, credentials are removed from the keychain.
