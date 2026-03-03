#!/bin/bash
while true; do
  read -n 1 char
  if [[ "$char" == "q" ]]; then
    break
  fi
  printf "%x\n" "'$char"
done
