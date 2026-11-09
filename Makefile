ENV_FILE := .env
ELF_PAYLOAD := target/x86_64-sie-ps5/release/wireguard-client-ps5
CARGO := cargo-ps5

.SILENT: ${ENV_FILE}

# .env2: PS5_TEST := $(shell read -rp "Enter PS5 hostname/IP:" PS5_HOST; echo "PS5_HOST")
${ENV_FILE}:
	echo Creating ${ENV_FILE}

	echo "# BEGIN - PS5 remote testing vars" > ${ENV_FILE}
	read -r -p "Enter PS5 hostname/IP: " VAL; echo "PS5_HOST=$$VAL" >> ${ENV_FILE}
	read -r -p "Enter PS5 port: " VAL; echo "PS5_PORT=$${VAL:-9021}" >> ${ENV_FILE}
	echo "# END - PS5 remote testing vars" >> ${ENV_FILE}

	echo ${ENV_FILE} created

.PHONY: ${ELF_PAYLOAD}
${ELF_PAYLOAD}:
	${CARGO} build --release

build_release: ${ELF_PAYLOAD}

.PHONY: test_elf
test_elf: .env build_release
	prospero-deploy ${ELF_PAYLOAD}
