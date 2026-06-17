#!/usr/bin/env python3
"""
Train an LSTM channel predictor and export to ONNX for Coppa.

Generates synthetic channel quality data mimicking HF propagation,
trains a small LSTM model, and exports it as ONNX for use with
the coppa-ml crate's tract backend.

Usage:
    python train_predictor.py --output predictor.onnx --epochs 100

Requirements:
    pip install torch numpy onnx
"""

import argparse
import numpy as np

try:
    import torch
    import torch.nn as nn
    import torch.optim as optim
    HAS_TORCH = True
except ImportError:
    HAS_TORCH = False


def generate_synthetic_channel(num_samples: int = 10000, seed: int = 42) -> np.ndarray:
    """Generate synthetic HF channel quality data.

    Simulates slow fading, sudden propagation changes, and noise.
    Output is SNR in dB, typically ranging from -5 to 30 dB.
    """
    rng = np.random.RandomState(seed)

    t = np.arange(num_samples) / 100.0  # 100 Hz observation rate

    # Base propagation: slow sinusoidal variation (ionospheric)
    base = 15.0 + 8.0 * np.sin(2 * np.pi * t / 300.0)  # 5-minute period

    # Medium-term fading: Rayleigh-like
    fading = 3.0 * np.sin(2 * np.pi * t / 30.0) * np.sin(2 * np.pi * t / 7.0)

    # Sudden propagation changes (skip changes)
    changes = np.zeros(num_samples)
    for i in range(num_samples // 2000):
        pos = rng.randint(0, num_samples)
        width = rng.randint(100, 500)
        amplitude = rng.uniform(-10, 10)
        start = max(0, pos - width // 2)
        end = min(num_samples, pos + width // 2)
        changes[start:end] += amplitude

    # Measurement noise
    noise = rng.normal(0, 1.5, num_samples)

    snr = base + fading + changes + noise
    return snr.astype(np.float32)


def create_sequences(data: np.ndarray, seq_len: int = 32, pred_ahead: int = 5):
    """Create training sequences: input seq_len samples, predict pred_ahead ahead."""
    X, y = [], []
    for i in range(len(data) - seq_len - pred_ahead):
        X.append(data[i:i + seq_len])
        y.append(data[i + seq_len + pred_ahead - 1])
    return np.array(X), np.array(y)


if HAS_TORCH:
    class ChannelLSTM(nn.Module):
        """Small LSTM for channel prediction."""

        def __init__(self, input_size=1, hidden_size=32, num_layers=2):
            super().__init__()
            self.lstm = nn.LSTM(input_size, hidden_size, num_layers, batch_first=True)
            self.fc = nn.Linear(hidden_size, 1)

        def forward(self, x):
            # x shape: (batch, seq_len, 1)
            lstm_out, _ = self.lstm(x)
            # Take the last time step
            last = lstm_out[:, -1, :]
            return self.fc(last).squeeze(-1)


def train(args):
    if not HAS_TORCH:
        print("PyTorch not available. Install with: pip install torch")
        print("Generating synthetic data only...")
        data = generate_synthetic_channel(10000)
        np.save("synthetic_channel.npy", data)
        print(f"Saved synthetic data to synthetic_channel.npy ({len(data)} samples)")
        return

    print("Generating synthetic channel data...")
    data = generate_synthetic_channel(args.samples, seed=args.seed)

    print(f"Data stats: mean={data.mean():.1f} dB, std={data.std():.1f} dB, "
          f"min={data.min():.1f} dB, max={data.max():.1f} dB")

    # Create sequences
    X, y = create_sequences(data, seq_len=args.seq_len, pred_ahead=args.pred_ahead)
    X = X.reshape(-1, args.seq_len, 1)

    # Normalize
    mean, std = X.mean(), X.std()
    X = (X - mean) / std
    y = (y - mean) / std

    # Split train/val
    split = int(0.8 * len(X))
    X_train, X_val = torch.FloatTensor(X[:split]), torch.FloatTensor(X[split:])
    y_train, y_val = torch.FloatTensor(y[:split]), torch.FloatTensor(y[split:])

    print(f"Training: {len(X_train)} sequences, Validation: {len(X_val)} sequences")

    # Create model
    model = ChannelLSTM(hidden_size=args.hidden_size, num_layers=args.num_layers)
    optimizer = optim.Adam(model.parameters(), lr=args.lr)
    criterion = nn.MSELoss()

    # Train
    batch_size = args.batch_size
    best_val_loss = float('inf')

    for epoch in range(args.epochs):
        model.train()
        epoch_loss = 0
        n_batches = 0

        indices = torch.randperm(len(X_train))
        for i in range(0, len(X_train), batch_size):
            batch_idx = indices[i:i + batch_size]
            batch_X = X_train[batch_idx]
            batch_y = y_train[batch_idx]

            optimizer.zero_grad()
            pred = model(batch_X)
            loss = criterion(pred, batch_y)
            loss.backward()
            optimizer.step()

            epoch_loss += loss.item()
            n_batches += 1

        # Validation
        model.eval()
        with torch.no_grad():
            val_pred = model(X_val)
            val_loss = criterion(val_pred, y_val).item()

        if (epoch + 1) % 10 == 0:
            print(f"Epoch {epoch + 1}/{args.epochs}: "
                  f"train_loss={epoch_loss / n_batches:.4f}, val_loss={val_loss:.4f}")

        if val_loss < best_val_loss:
            best_val_loss = val_loss
            torch.save(model.state_dict(), "best_model.pt")

    print(f"\nBest validation loss: {best_val_loss:.4f}")

    # Export to ONNX
    model.load_state_dict(torch.load("best_model.pt"))
    model.eval()

    dummy_input = torch.randn(1, args.seq_len, 1)
    torch.onnx.export(
        model,
        dummy_input,
        args.output,
        input_names=["channel_history"],
        output_names=["predicted_snr"],
        dynamic_axes={
            "channel_history": {0: "batch_size"},
            "predicted_snr": {0: "batch_size"},
        },
        opset_version=11,
    )

    print(f"Exported ONNX model to {args.output}")

    # Save normalization params for inference
    np.savez(
        args.output.replace(".onnx", "_params.npz"),
        mean=mean, std=std,
        seq_len=args.seq_len,
        pred_ahead=args.pred_ahead,
    )
    print(f"Saved normalization params to {args.output.replace('.onnx', '_params.npz')}")


def main():
    parser = argparse.ArgumentParser(description="Train Coppa channel predictor")
    parser.add_argument("--output", default="predictor.onnx", help="Output ONNX file")
    parser.add_argument("--epochs", type=int, default=100)
    parser.add_argument("--batch-size", type=int, default=64)
    parser.add_argument("--lr", type=float, default=0.001)
    parser.add_argument("--hidden-size", type=int, default=32)
    parser.add_argument("--num-layers", type=int, default=2)
    parser.add_argument("--seq-len", type=int, default=32)
    parser.add_argument("--pred-ahead", type=int, default=5)
    parser.add_argument("--samples", type=int, default=10000)
    parser.add_argument("--seed", type=int, default=42)

    args = parser.parse_args()
    train(args)


if __name__ == "__main__":
    main()
