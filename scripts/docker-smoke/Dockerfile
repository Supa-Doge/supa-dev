FROM ubuntu:22.04
RUN apt-get -o Acquire::http::Timeout=30 -o Acquire::Retries=2 update && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::http::Timeout=30 -o Acquire::Retries=2 install -y --no-install-recommends \
    ca-certificates curl git xz-utils unzip tar xvfb xterm xfonts-base x11-utils xauth \
    libgl1-mesa-dri libgl1 libegl1 libx11-6 libxi6 libxcursor1 libxrandr2 \
    libxinerama1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
    libdbus-1-3 libatomic1 libgomp1 fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 1000 tester && mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
USER tester
ENV HOME=/home/tester SUPA_DEV_HOME=/home/tester/supa-data DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
WORKDIR /home/tester
CMD ["sleep", "infinity"]
