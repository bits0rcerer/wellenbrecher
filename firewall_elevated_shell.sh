#/bin/bash

capsh --caps="cap_net_admin+eip cap_setpcap,cap_setuid,cap_setgid+ep" --keep=1 --user="$1" --addamb=cap_net_admin -- -c "$SHELL"
