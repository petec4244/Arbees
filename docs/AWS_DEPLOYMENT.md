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
- `ENV`: `production`

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
