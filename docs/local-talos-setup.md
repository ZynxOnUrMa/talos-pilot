# Local Talos Cluster Setup Guide

This guide documents how to set up a local Talos Linux cluster using Docker for learning and development purposes.

## Prerequisites

- Linux (x86_64)
- Docker installed and running

## Step 1: Install talosctl

`talosctl` is the CLI tool for managing Talos clusters.

```bash
# Create local bin directory
mkdir -p ~/.local/bin

# Download and install talosctl
curl -sLO https://github.com/siderolabs/talos/releases/latest/download/talosctl-linux-amd64
chmod +x talosctl-linux-amd64
mv talosctl-linux-amd64 ~/.local/bin/talosctl

# Verify installation
~/.local/bin/talosctl version --client
```

## Step 2: Install kubectl

`kubectl` is needed to interact with the Kubernetes cluster running on Talos.

```bash
# Download and install kubectl
curl -sLO "https://dl.k8s.io/release/$(curl -sL https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
chmod +x kubectl
mv kubectl ~/.local/bin/kubectl

# Verify installation
~/.local/bin/kubectl version --client
```

## Step 3: Add ~/.local/bin to PATH

Add this line to your `~/.bashrc` or `~/.zshrc`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then reload your shell:

```bash
source ~/.bashrc  # or source ~/.zshrc
```

## Step 4: Load Required Kernel Modules

The Flannel CNI (Container Network Interface) requires the `br_netfilter` kernel module:

```bash
# Load the module
sudo modprobe br_netfilter

# Enable iptables for bridged traffic
sudo sysctl net.bridge.bridge-nf-call-iptables=1
```

To make this persistent across reboots:

```bash
# Add module to load at boot
echo "br_netfilter" | sudo tee /etc/modules-load.d/br_netfilter.conf

# Make sysctl setting persistent
echo "net.bridge.bridge-nf-call-iptables=1" | sudo tee /etc/sysctl.d/99-kubernetes.conf
```

## Step 5: Create the Talos Cluster

Create a single-node cluster using Docker:

```bash
talosctl cluster create docker --name talos-pilot --workers 0
```

This command:
- Creates a Docker network named `talos-pilot`
- Pulls the Talos container image
- Starts a control plane node as a Docker container
- Bootstraps the Kubernetes cluster
- Generates configuration files in `~/.talos/`

### Cluster Options

- `--name <name>`: Name of the cluster (default: talos-default)
- `--workers <n>`: Number of worker nodes (default: 1)
- `--kubernetes-version <version>`: Specific Kubernetes version
- `--memory-controlplanes <size>`: Memory limit for control plane (default: 2GiB)
- `--cpus-controlplanes <n>`: CPU allocation for control plane (default: 2.0)

## Step 6: Configure kubectl

Fetch the kubeconfig from the cluster:

```bash
talosctl --nodes 10.5.0.2 kubeconfig ~/.talos/clusters/talos-pilot/kubeconfig --force
```

Set the KUBECONFIG environment variable:

```bash
export KUBECONFIG=~/.talos/clusters/talos-pilot/kubeconfig
```

Or add it to your shell profile for persistence.

## Verifying the Cluster

### Check Talos Cluster Status

```bash
talosctl cluster show --name talos-pilot
```

### Check Kubernetes Nodes

```bash
kubectl get nodes
```

Expected output:
```
NAME                         STATUS   ROLES           AGE   VERSION
talos-pilot-controlplane-1   Ready    control-plane   Xm    v1.35.0
```

### Check System Pods

```bash
kubectl get pods -A
```

All pods should eventually be in `Running` state:
- `coredns-*` (2 replicas)
- `kube-apiserver-*`
- `kube-controller-manager-*`
- `kube-scheduler-*`
- `kube-proxy-*`
- `kube-flannel-*`

## Using talosctl

### View Talos Dashboard

```bash
talosctl --nodes 10.5.0.2 dashboard
```

### Check Talos Services

```bash
talosctl --nodes 10.5.0.2 services
```

### View Talos Logs

```bash
talosctl --nodes 10.5.0.2 logs kubelet
```

### Get Cluster Members

```bash
talosctl --nodes 10.5.0.2 get members
```

## Troubleshooting

### Flannel Pod in Error State

If the `kube-flannel` pod is in Error state, ensure the kernel module is loaded:

```bash
lsmod | grep br_netfilter
```

If not loaded, run:

```bash
sudo modprobe br_netfilter
sudo sysctl net.bridge.bridge-nf-call-iptables=1
```

Then restart the flannel pod:

```bash
kubectl delete pod -n kube-system -l app=flannel
```

### CoreDNS Stuck in ContainerCreating

This usually means Flannel networking isn't working. Fix Flannel first (see above).

### Check Docker Containers

```bash
docker ps --filter "name=talos"
```

## Cluster Management

### Stop the Cluster

The Docker containers will stop when Docker stops, or you can stop them manually:

```bash
docker stop talos-pilot-controlplane-1
```

### Start the Cluster Again

```bash
docker start talos-pilot-controlplane-1
```

### Destroy the Cluster

```bash
talosctl cluster destroy --name talos-pilot
```

This removes:
- All Docker containers for the cluster
- The Docker network
- State files in `~/.talos/clusters/talos-pilot/`

## File Locations

| File | Description |
|------|-------------|
| `~/.local/bin/talosctl` | talosctl binary |
| `~/.local/bin/kubectl` | kubectl binary |
| `~/.talos/config` | talosctl configuration (contexts, credentials) |
| `~/.talos/clusters/talos-pilot/` | Cluster state directory |
| `~/.talos/clusters/talos-pilot/kubeconfig` | Kubernetes kubeconfig |

## Versions Installed

- talosctl: v1.12.1
- kubectl: v1.35.0
- Talos Linux: v1.12.1
- Kubernetes: v1.35.0
