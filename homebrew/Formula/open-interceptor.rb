class OpenInterceptor < Formula
  desc "Local proxy that auto-routes Claude Code API traffic to the right provider"
  homepage "https://github.com/ragnito/open-interceptor"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ragnito/open-interceptor/releases/download/v#{version}/open-interceptor-darwin-arm64"
      sha256 "REPLACE_WITH_ARM64_SHA256"
    else
      url "https://github.com/ragnito/open-interceptor/releases/download/v#{version}/open-interceptor-darwin-x64"
      sha256 "REPLACE_WITH_X64_SHA256"
    end
  end

  def install
    if Hardware::CPU.arm?
      bin.install "open-interceptor-darwin-arm64" => "open-interceptor"
    else
      bin.install "open-interceptor-darwin-x64" => "open-interceptor"
    end
  end

  def caveats
    <<~EOS
      Create a config file first:
        mkdir -p ~/.config/open-interceptor
        cp config.yaml.example ~/.config/open-interceptor/config.yaml
        # edit ~/.config/open-interceptor/config.yaml with your providers

      Install and start the background daemon:
        open-interceptor start --install

      Then set in your shell profile (~/.zshrc, ~/.bashrc):
        export ANTHROPIC_BASE_URL=http://127.0.0.1:3300

      For Claude Code model picker integration, also add:
        export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1
    EOS
  end

  test do
    system "#{bin}/open-interceptor", "config", "--config", testpath/"dummy.yaml"
  end
end
