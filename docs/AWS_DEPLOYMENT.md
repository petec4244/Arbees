# Arbitrage Betting System (Arbees) - AWS Deployment Guide

This guide details the steps to deploy the Arbees platform to AWS using a modern, containerized architecture.

## 1. Architecture Overview

```mermaid
graph TD
    User[User/Browser] -->|HTTPS| ALB[Application LoadBalancer]
    ALB -->|/api/*| API[API Service (Fargate)]
    ALB -->|/*| Frontend[Frontend Service (Fargate)]
    
    subgraph "US-East-1 (Virginia)"
        subgraph "Core Services"
            API --> RDS[(TimescaleDB RDS)]
            API --> Redis[(ElastiCache Redis)]
            Orch[Orchestrator] --> Redis
            Core[Strategy Engine] --> Redis
            Core -->|Game Data| ESPN[ESPN API]
        end
        
        subgraph "US Market Agents"
            KalshiGW[Kalshi Gateway] -->|Price/Trade| Kalshi[Kalshi API]
            KalshiGW <-->|Redis Pub/Sub| Redis
        end
    end
    
    subgraph "EU-Central-1 (Frankfurt)"
        subgraph "EU Market Agents"
            PolyGW[Polymarket Gateway] -->|Price/Trade| Poly[Polymarket API]
            # Connecting to US Redis via VPC Peering / Transit Gateway
            PolyGW <-->|Redis Pub/Sub| Redis
        end
    end
    
    subgraph "Management"

        GH[GitHub Actions] -->|Push Image| ECR[Elastic Container Registry]
        GH -->|Deploy| ECS[ECS Cluster]
    end
```

## 2. Prerequisites

- AWS Account with Administrator access
- AWS CLI installed and configured
- Docker installed locally
- Domain name (e.g., `arbees.io`) managed in Route53 (optional but recommended)

## 3. Infrastructure Setup (Terraform / Console)

### 3.1 Networking (VPC)
- Create a VPC with 2 Public Subnets and 2 Private Subnets.
- **NAT Gateway**: Required for private subnets to access external APIs (ESPN, Kalshi, etc.).

### 3.2 Database (TimescaleDB)
- **Service**: RDS for PostgreSQL (or EC2 if using self-managed TimescaleDB image).
- **Engine**: PostgreSQL 14+ with TimescaleDB extension enabled.
- **Instance Type**: `t3.medium` (minimum for production).
- **Security Group**: Allow ingress on port 5432 from the VPC CIDR only.

### 3.3 Cache & Messaging (Redis)
- **Service**: Amazon ElastiCache for Redis.
- **Mode**: Cluster mode disabled (simple primary/replica) is sufficient.
- **Instance Type**: `cache.t3.micro` or `small`.
- **Security Group**: Allow ingress on port 6379 from VPC CIDR.
- **Location**: MUST be in **US-East-1** to be co-located with the Strategy Core and Kalshi Gateway. The EU Gateway will connect to this instance remotely.

### 3.4 Distributed Market Gateways (Multi-Region)
To maximize execution speed and regulatory compliance, we deploy "Agents" close to their respective exchanges.

#### A. US-East-1 (Home Base)
- **Kalshi Gateway**: Specialized container running `KalshiClient`. 
    - **Role**: Polls/Streams prices, executes orders locally (low latency to Kalshi servers in Virginia).
    - **Connection**: Connects to the central Redis bus.

#### B. EU-Central-1 (Remote Outpost)
- **Polymarket Gateway**: Specialized container running `PolymarketClient`.
    - **Role**: Polls/Streams prices, executes orders locally (low latency to Polygon/European nodes).
    - **Networking**: Connects back to US-East-1 Redis via **VPC Peering** or **Transit Gateway**.
    - **Latency**: The ~80ms Atlantic hop happens *between* the Gateway and the Strategy Engine. 
        - This means we receive price updates ~80ms delayed.
        - But once a decision is made, the execution happens locally in EU (robustness).

## 4. Container Registry (ECR)

Create repositories for each service:
```bash
aws ecr create-repository --repository-name arbees/frontend
aws ecr create-repository --repository-name arbees/api
aws ecr create-repository --repository-name arbees/orchestrator
aws ecr create-repository --repository-name arbees/strategy-engine
aws ecr create-repository --repository-name arbees/market-gateway-kalshi
aws ecr create-repository --repository-name arbees/market-gateway-polymarket
```

## 5. ECS Configuration (Fargate)

### 5.1 Task Definitions
For each service, define a Task Definition using Fargate launch type.

**Environment Variables (via Secrets Manager/Parameter Store):**
- `DATABASE_URL`: `postgresql://user:pass@rds-endpoint:5432/arbees`
- `REDIS_URL`: `redis://elasticache-endpoint:6379/0`
- `KALSHI_API_KEY`, `KALSHI_PRIVATE_KEY`
- `POLYMARKET_API_KEY`
- `POLYMARKET_PROXY_URL`: `http://user:pass@eu-proxy-ip:3128`
- `POLYMARKET_API_KEY`
- `POLYMARKET_PROXY_URL`: `http://user:pass@eu-proxy-ip:3128`
- `ENV`: `production`

**Latency Tuning:**
- `POLL_INTERVAL`: `1.0` (Seconds between ESPN checks)
- `CRUNCH_TIME_INTERVAL`: `0.5` (Faster polling for late game)
- `MARKET_DATA_TTL`: `4.0` (Max age of market data before skipping signal)
- `SYNC_DELTA_TOLERANCE`: `2.0` (Max allowed diff between Game vs Market timestamps)

### 5.2 Services
1.  **Frontend Service**:
    - Port: 80
    - Load Balancer: Associate with Public ALB.
2.  **API Service**:
    - Port: 8000
    - Load Balancer: Associate with Public ALB (path `/api/*`).
3.  **Orchestrator Service**:
    - No Load Balancer needed.
4.  **GameShard Service**:
    - No Load Balancer needed.
    - **Scaling**: Configure Auto Scaling based on CPU/Memory usage.

### 5.3 Domain & SSL (Custom URL)
To serve the frontend at a custom domain (e.g., `app.your-site.com`):
1.  **SSL Certificate**: Request a public certificate in **AWS Certificate Manager (ACM)** for your domain.
2.  **Load Balancer (ALB)**: Add an HTTPS listener (port 443) using the ACM certificate.
3.  **DNS Mapping**:
    - If you use AWS Route53: Create an **A Record** alias pointing to the ALB.
    - If you use External DNS (GoDaddy, Namecheap): Create a **CNAME Record** pointing `app` to the ALB's DNS name (e.g., `arbees-alb-1234.us-east-1.elb.amazonaws.com`).

## 6. CI/CD Pipeline (GitHub Actions)

Create `.github/workflows/deploy.yml`:

```yaml
name: Deploy to AWS

on:
  push:
    branches: [ main ]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v2
    
    - name: Configure AWS credentials
      uses: aws-actions/configure-aws-credentials@v1
      with:
        aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
        aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
        aws-region: us-east-1

    - name: Login to Amazon ECR
      id: login-ecr
      uses: aws-actions/amazon-ecr-login@v1

    - name: Build, tag, and push API image
      env:
        ECR_REGISTRY: ${{ steps.login-ecr.outputs.registry }}
        ECR_REPOSITORY: arbees/api
        IMAGE_TAG: ${{ github.sha }}
      run: |
        docker build -t $ECR_REGISTRY/$ECR_REPOSITORY:$IMAGE_TAG -f services/api/Dockerfile .
        docker push $ECR_REGISTRY/$ECR_REPOSITORY:$IMAGE_TAG

    - name: Update ECS Service
      run: |
        aws ecs update-service --cluster arbees-cluster --service api-service --force-new-deployment
```

## 7. Migration Strategy

1.  **Schema Migration**: Run `flyway` or python migration scripts from an ephemeral container or strictly from the deployments pipeline before updating the API service.
2.  **Seed Data**: Ensure initial seed data is loaded into RDS.

## 8. Monitoring

- **CloudWatch Logs**: All containers stream stdout/stderr to CloudWatch.
- **CloudWatch Metrics**: CPU, Memory, RDS Connections.
- **Cost Estimation**:
    - ALB: ~$16/mo
    - NAT Gateway: ~$32/mo (Use single NAT for dev/test to save costs)
    - Fargate: Pay per vCPU/GB usage.
    - RDS/Redis: Instance hourly rates.

---

## 9. Fargate Limitations for VPN (P3-3)

### The Problem

AWS Fargate does **NOT** support VPN containers because:

1. **NET_ADMIN capability not allowed**: Fargate's security model prohibits the `NET_ADMIN` Linux capability required to create virtual network interfaces.

2. **No /dev/net/tun access**: VPN software (OpenVPN, WireGuard) requires the TUN/TAP device, which is not exposed in Fargate.

3. **No privileged containers**: Fargate doesn't support `--privileged` mode.

**Impact**: The `polymarket_monitor` service cannot run through a VPN container (gluetun) on Fargate.

### Alternatives

| Option | Pros | Cons | Best For |
|--------|------|------|----------|
| **EC2 for VPN** | Full VPN support, same Docker pattern | Must manage EC2 instance | Production |
| **EU Proxy** | All services on Fargate | Added latency, single point of failure | Dev/Test |
| **EU Region Deploy** | No VPN needed, low Polymarket latency | Cross-region Redis complexity | High volume |

### Recommended: Hybrid Approach

Deploy most services on **Fargate** (us-east-1), but run **polymarket_monitor + gluetun VPN** on a small **EC2 instance** (eu-central-1):

```
┌──────────────────────────────────────────────────────────────┐
│                      us-east-1 (Fargate)                     │
│  orchestrator, game_shard, signal_processor, execution,     │
│  position_tracker, kalshi_monitor, api, frontend            │
│                           │                                  │
│                    Redis (ElastiCache)                       │
└──────────────────────────────────────────────────────────────┘
                            │
                     Cross-region
                            │
┌──────────────────────────────────────────────────────────────┐
│                    eu-central-1 (EC2 t3.small)               │
│  ┌─────────────┐    ┌─────────────────────────────┐         │
│  │   gluetun   │───▶│    polymarket_monitor       │         │
│  │    (VPN)    │    │  (network_mode: container)  │         │
│  └─────────────┘    └─────────────────────────────┘         │
└──────────────────────────────────────────────────────────────┘
```

### EC2 Task Definition for VPN

```json
{
  "family": "arbees-polymarket-vpn",
  "requiresCompatibilities": ["EC2"],
  "networkMode": "bridge",
  "containerDefinitions": [
    {
      "name": "vpn",
      "image": "qmcgaw/gluetun:latest",
      "essential": true,
      "portMappings": [{"containerPort": 8888, "hostPort": 8888}],
      "linuxParameters": {
        "capabilities": {"add": ["NET_ADMIN"]},
        "devices": [{"hostPath": "/dev/net/tun", "containerPath": "/dev/net/tun"}]
      },
      "environment": [
        {"name": "VPN_SERVICE_PROVIDER", "value": "nordvpn"},
        {"name": "SERVER_COUNTRIES", "value": "Netherlands,Germany,Belgium"}
      ],
      "secrets": [
        {"name": "OPENVPN_USER", "valueFrom": "arn:aws:secretsmanager:..."},
        {"name": "OPENVPN_PASSWORD", "valueFrom": "arn:aws:secretsmanager:..."}
      ],
      "healthCheck": {
        "command": ["CMD", "wget", "-q", "--spider", "http://ipinfo.io/json"],
        "interval": 30,
        "timeout": 10,
        "retries": 5
      }
    },
    {
      "name": "polymarket_monitor",
      "image": "${ECR_REGISTRY}/arbees-polymarket_monitor:latest",
      "essential": true,
      "links": ["vpn:vpn"],
      "networkMode": "container:vpn",
      "dependsOn": [{"containerName": "vpn", "condition": "HEALTHY"}],
      "environment": [
        {"name": "REDIS_URL", "value": "redis://arbees-redis.xxx.use1.cache.amazonaws.com:6379"}
      ]
    }
  ]
}
```

### Cost Impact

| Component | Monthly Cost |
|-----------|--------------|
| Fargate services (6x 0.25vCPU/512MB) | ~$30 |
| EC2 t3.small (eu-central-1) | ~$15 |
| Cross-region data transfer | ~$5 |
| **VPN overhead total** | **~$20** |

### Monitoring VPN Health

```bash
# CloudWatch alarm for VPN container health
aws cloudwatch put-metric-alarm \
  --alarm-name arbees-vpn-health \
  --metric-name CPUUtilization \
  --namespace AWS/ECS \
  --dimensions Name=ServiceName,Value=polymarket-vpn \
  --statistic Average \
  --period 60 \
  --evaluation-periods 3 \
  --threshold 0 \
  --comparison-operator LessThanOrEqualToThreshold \
  --treat-missing-data breaching \
  --alarm-actions arn:aws:sns:us-east-1:xxx:arbees-alerts
```

### Failover Configuration

The gluetun VPN container supports automatic server failover:

```yaml
# docker-compose.yml (for reference)
vpn:
  environment:
    - SERVER_COUNTRIES=Netherlands,Germany,Belgium,France
    - PUBLICIP_API=ipinfo.io
    - PUBLICIP_PERIOD=60s
  deploy:
    restart_policy:
      condition: any
      delay: 10s
      max_attempts: 10
```

This configuration is already in place in the local docker-compose.yml and should be replicated in the EC2 deployment.
